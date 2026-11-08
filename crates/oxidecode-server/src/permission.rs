use crate::error::ApiResult;
use crate::state::{AppState, PermissionAnswer};
use async_trait::async_trait;
use oxidecode_core::config::PermissionRule;
use oxidecode_core::ids;
use oxidecode_core::session::{LocationRef, SessionInfo};
use oxidecode_core::store::PermissionInfo;
use oxidecode_tools::{Approval, PermissionPolicy, PermissionRequest};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::watch;

/// Permission policy bound to one agent run.
pub struct SessionPolicy {
    pub state: AppState,
    pub session_id: String,
    pub directory: String,
    pub agent: String,
    pub rules: Vec<PermissionRule>,
    pub abort: watch::Receiver<bool>,
}

fn rule_matches(rule: &PermissionRule, action: &str, resource: &str) -> bool {
    let action_matches = rule.action == "*" || rule.action == action;
    if !action_matches {
        return false;
    }
    if rule.resource == "*" || rule.resource.is_empty() {
        return true;
    }
    wildcard_match(&rule.resource, resource)
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut p, mut v) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            mark = v;
            p += 1;
        } else if let Some(star_index) = star {
            p = star_index + 1;
            mark += 1;
            v = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

impl SessionPolicy {
    pub fn new(
        state: AppState,
        session: &SessionInfo,
        config: &oxidecode_core::config::LoadedConfig,
        abort: watch::Receiver<bool>,
    ) -> Self {
        let mut rules: Vec<PermissionRule> = config.config.permissions.clone();
        if let Some(agent) = config.config.agents.get(&session.agent.clone().unwrap_or_default()) {
            if let Some(extra) = &agent.permissions {
                rules.extend(extra.iter().cloned());
            }
        }
        if let Some(extra) = session
            .permissions
            .as_ref()
            .and_then(|value| serde_json::from_value::<Vec<PermissionRule>>(value.clone()).ok())
        {
            rules.extend(extra);
        }
        Self {
            state,
            session_id: session.id.clone(),
            directory: session.location.directory.clone(),
            agent: session.agent.clone().unwrap_or_else(|| "build".to_string()),
            rules,
            abort,
        }
    }

    fn default_rule(&self, action: &str) -> Approval {
        if self.agent == "plan" {
            return match action {
                "edit" | "write" | "bash" | "patch" | "task" => Approval::Deny,
                _ => Approval::Allow,
            };
        }
        match action {
            "read" | "glob" | "grep" | "list" | "todoread" | "todowrite" | "webfetch" => {
                Approval::Allow
            }
            "bash" | "edit" | "write" | "patch" | "task" => Approval::Ask,
            _ => Approval::Ask,
        }
    }
}

#[async_trait]
impl PermissionPolicy for SessionPolicy {
    async fn evaluate(&self, request: &PermissionRequest) -> Approval {
        let mut decision = self.default_rule(&request.action);
        for rule in &self.rules {
            let applies = if request.resources.is_empty() {
                rule_matches(rule, &request.action, "")
            } else {
                request
                    .resources
                    .iter()
                    .any(|resource| rule_matches(rule, &request.action, resource))
            };
            if applies {
                decision = match rule.effect.as_str() {
                    "allow" => Approval::Allow,
                    "deny" => Approval::Deny,
                    _ => Approval::Ask,
                };
            }
        }
        decision
    }

    async fn ask(&self, request: &PermissionRequest) -> bool {
        let id = ids::permission();
        let permission = PermissionInfo {
            id: id.clone(),
            session_id: self.session_id.clone(),
            action: request.action.clone(),
            resources: request.resources.clone(),
            save: request.save.clone(),
            metadata: request.metadata.clone(),
            source: Some(json!({ "type": "agent", "agent": self.agent })),
            message: None,
            decision: None,
        };
        if self
            .state
            .store()
            .create_permission(&permission)
            .is_err()
        {
            return false;
        }

        let receiver = self.state.register_approval(&id);
        self.state.bus().emit_session(
            "permission.asked",
            &self.directory,
            json!({
                "id": id,
                "sessionID": self.session_id,
                "action": request.action,
                "resources": request.resources,
                "save": request.save,
                "metadata": request.metadata,
                "source": permission.source,
            }),
        );

        let mut abort = self.abort.clone();
        let answer = tokio::select! {
            answer = receiver => answer.ok(),
            _ = async {
                while !*abort.borrow() {
                    if abort.changed().await.is_err() {
                        break;
                    }
                }
            } => None,
        };

        let Some(answer) = answer else {
            self.state.cancel_approval(&id);
            let _ = self.state.store().reply_permission(
                &self.session_id,
                &id,
                "reject",
            );
            return false;
        };

        let _ = self.state.store().reply_permission(
            &self.session_id,
            &id,
            if answer.allow { "once" } else { "reject" },
        );

        if answer.allow && answer.always {
            persist_rule(
                &self.state,
                &self.session_id,
                &request.action,
                &request.save,
            );
        }

        self.state.bus().emit_session(
            "permission.replied",
            &self.directory,
            json!({
                "sessionID": self.session_id,
                "requestID": id,
                "reply": if answer.allow { "always" } else { "reject" },
            }),
        );

        answer.allow
    }
}

pub fn persist_rule(
    state: &AppState,
    session_id: &str,
    action: &str,
    resources: &[String],
) {
    let Ok(Some(mut session)) = state.store().get_session(session_id) else {
        return;
    };
    let mut rules: Vec<PermissionRule> = session
        .permissions
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    if resources.is_empty() {
        rules.push(PermissionRule {
            action: action.to_string(),
            resource: "*".to_string(),
            effect: "allow".to_string(),
        });
    } else {
        for resource in resources {
            rules.push(PermissionRule {
                action: action.to_string(),
                resource: resource.clone(),
                effect: "allow".to_string(),
            });
        }
    }
    session.permissions = Some(serde_json::to_value(rules).unwrap_or(Value::Null));
    let _ = state.store().update_session(&session);
}

/// Applies a permission reply coming from the HTTP API.
pub fn reply(
    state: &AppState,
    session_id: &str,
    request_id: &str,
    decision: &str,
    message: Option<String>,
) -> ApiResult<()> {
    let Some(permission) = state
        .store()
        .get_permission(session_id, request_id)
        .map_err(crate::error::ApiError::from)?
    else {
        return Err(crate::error::ApiError::permission_not_found(request_id));
    };

    let allow = decision == "once" || decision == "always";
    let always = decision == "always";
    let answer = PermissionAnswer {
        allow,
        always,
        message: message.clone(),
    };
    state.resolve_approval(request_id, answer);
    let _ = state.store().reply_permission(session_id, request_id, decision);
    if allow && always {
        persist_rule(state, session_id, &permission.action, &permission.save);
    }

    if let Some(session) = state.store().get_session(session_id).ok().flatten() {
        state.bus().emit_session(
            "permission.replied",
            &session.location.directory,
            json!({
                "sessionID": session_id,
                "requestID": request_id,
                "reply": decision,
            }),
        );
    }
    Ok(())
}

pub fn session_ruleset(session: &SessionInfo) -> Vec<PermissionRule> {
    session
        .permissions
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

pub fn policy_for(
    state: &AppState,
    session: &SessionInfo,
    config: &Arc<oxidecode_core::config::LoadedConfig>,
    abort: watch::Receiver<bool>,
) -> Arc<dyn PermissionPolicy> {
    Arc::new(SessionPolicy::new(
        state.clone(),
        session,
        config,
        abort,
    ))
}

pub fn location_of(directory: &str) -> LocationRef {
    LocationRef::new(directory)
}
