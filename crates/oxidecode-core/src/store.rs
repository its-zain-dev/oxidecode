use crate::db::Db;
use crate::event::{Event, EventBus};
use crate::ids;
use crate::session::{LocationRef, Message, SessionInfo};
use crate::vcs;
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectTime {
    pub created: i64,
    pub updated: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub id: String,
    pub directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<String>,
    pub time: ProjectTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionInfo {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub action: String,
    pub resources: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub save: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

#[derive(Clone)]
pub struct Store {
    pub db: Arc<Db>,
    pub bus: EventBus,
}

impl Store {
    pub fn new(db: Arc<Db>, bus: EventBus) -> Self {
        Self { db, bus }
    }

    pub fn ensure_project(&self, directory: &Path) -> Result<ProjectInfo> {
        let canonical = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        let root = vcs::git_root(&canonical).unwrap_or_else(|| canonical.clone());
        let directory = root.display().to_string();
        let id = ids::project_id(&directory);
        if let Some(project) = self.get_project(&id)? {
            return Ok(project);
        }
        let now = ids::now_ms();
        let project = ProjectInfo {
            id: id.clone(),
            directory: directory.clone(),
            name: root
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_string()),
            vcs: if root.join(".git").exists() {
                Some("git".to_string())
            } else {
                None
            },
            time: ProjectTime {
                created: now,
                updated: now,
            },
        };
        let body = serde_json::to_string(&project)?;
        self.db.with(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO projects (id, directory, data_json, time_created, time_updated)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![project.id, project.directory, body, now, now],
            )
        })?;
        Ok(project)
    }

    pub fn get_project(&self, id: &str) -> Result<Option<ProjectInfo>> {
        let body: Option<String> = self.db.with(|connection| {
            connection
                .query_row(
                    "SELECT data_json FROM projects WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
        })?;
        Ok(body.and_then(|body| serde_json::from_str(&body).ok()))
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectInfo>> {
        let rows: Vec<String> = self.db.with(|connection| {
            let mut statement =
                connection.prepare("SELECT data_json FROM projects ORDER BY time_updated DESC")?;
            let values = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(values)
        })?;
        Ok(rows
            .into_iter()
            .filter_map(|body| serde_json::from_str(&body).ok())
            .collect())
    }

    pub fn create_session(&self, session: &SessionInfo) -> Result<()> {
        let body = serde_json::to_string(session)?;
        self.db.with(|connection| {
            connection.execute(
                "INSERT INTO sessions (id, project_id, parent_id, directory, title, time_created, time_updated, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    session.id,
                    session.project_id,
                    session.parent_id,
                    session.location.directory,
                    session.title,
                    session.time.created,
                    session.time.updated,
                    body
                ],
            )
        })?;
        self.bus.publish(Event::session(
            "session.created",
            &session.location.directory,
            json!({ "sessionID": session.id, "info": session }),
        ));
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionInfo>> {
        let body: Option<String> = self.db.with(|connection| {
            connection
                .query_row(
                    "SELECT data_json FROM sessions WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
        })?;
        Ok(body.and_then(|body| serde_json::from_str(&body).ok()))
    }

    pub fn session_exists(&self, id: &str) -> Result<bool> {
        let count: i64 = self.db.with(|connection| {
            connection.query_row(
                "SELECT COUNT(1) FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
        })?;
        Ok(count > 0)
    }

    pub fn list_sessions(
        &self,
        project_id: Option<&str>,
        directory: Option<&str>,
        search: Option<&str>,
        limit: usize,
        include_archived: bool,
        order: &str,
    ) -> Result<Vec<SessionInfo>> {
        let direction = if order.eq_ignore_ascii_case("asc") {
            "ASC"
        } else {
            "DESC"
        };
        let sql = format!(
            "SELECT data_json FROM sessions
             WHERE (?1 IS NULL OR project_id = ?1)
               AND (?2 IS NULL OR directory = ?2)
               AND (?3 IS NULL OR title LIKE ?3)
             ORDER BY time_updated {direction} LIMIT ?4"
        );
        let like = search.map(|value| format!("%{value}%"));
        let rows: Vec<String> = self.db.with(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let values = statement
                .query_map(
                    params![project_id, directory, like, limit as i64],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(values)
        })?;
        let mut sessions: Vec<SessionInfo> = rows
            .into_iter()
            .filter_map(|body| serde_json::from_str(&body).ok())
            .filter(|session: &SessionInfo| include_archived || session.time.archived.is_none())
            .collect();
        sessions.sort_by_key(|session| {
            if order.eq_ignore_ascii_case("asc") {
                session.time.updated
            } else {
                -session.time.updated
            }
        });
        Ok(sessions)
    }

    pub fn update_session(&self, session: &SessionInfo) -> Result<()> {
        let body = serde_json::to_string(session)?;
        self.db.with(|connection| {
            connection.execute(
                "UPDATE sessions SET title = ?1, time_updated = ?2, data_json = ?3 WHERE id = ?4",
                params![session.title, session.time.updated, body, session.id],
            )
        })?;
        self.bus.publish(Event::session(
            "session.updated",
            &session.location.directory,
            json!({ "sessionID": session.id, "info": session }),
        ));
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> Result<Option<SessionInfo>> {
        let session = self.get_session(id)?;
        if let Some(session) = &session {
            self.db.with(|connection| {
                connection.execute("DELETE FROM sessions WHERE id = ?1", params![id])
            })?;
            self.bus.publish(Event::session(
                "session.deleted",
                &session.location.directory,
                json!({ "sessionID": id }),
            ));
        }
        Ok(session)
    }

    pub fn append_message(&self, session_id: &str, message: &Message) -> Result<i64> {
        let body = serde_json::to_string(message)?;
        let created = message.created();
        let seq = self.db.with_mut(|connection| {
            let transaction = connection.transaction()?;
            let seq: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO messages (id, session_id, seq, time_created, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![message.id(), session_id, seq, created, body],
            )?;
            transaction.execute(
                "UPDATE sessions SET time_updated = ?1, data_json = json_set(data_json, '$.time.updated', ?1) WHERE id = ?2",
                params![created, session_id],
            )?;
            transaction.commit()?;
            Ok(seq)
        })?;
        Ok(seq)
    }

    pub fn update_message(&self, session_id: &str, message: &Message) -> Result<()> {
        let body = serde_json::to_string(message)?;
        let now = ids::now_ms();
        self.db.with(|connection| {
            connection.execute(
                "UPDATE messages SET data_json = ?1 WHERE id = ?2 AND session_id = ?3",
                params![body, message.id(), session_id],
            )?;
            connection.execute(
                "UPDATE sessions SET time_updated = ?1, data_json = json_set(data_json, '$.time.updated', ?1) WHERE id = ?2",
                params![now, session_id],
            )
        })?;
        Ok(())
    }

    pub fn delete_message(&self, session_id: &str, message_id: &str) -> Result<()> {
        self.db.with(|connection| {
            connection.execute(
                "DELETE FROM messages WHERE id = ?1 AND session_id = ?2",
                params![message_id, session_id],
            )
        })?;
        Ok(())
    }

    pub fn list_messages(
        &self,
        session_id: &str,
        limit: usize,
        order: &str,
        since_ms: Option<i64>,
    ) -> Result<Vec<Message>> {
        let direction = if order.eq_ignore_ascii_case("asc") {
            "ASC"
        } else {
            "DESC"
        };
        let sql = format!(
            "SELECT data_json FROM messages WHERE session_id = ?1 AND (?2 IS NULL OR time_created > ?2)
             ORDER BY seq {direction} LIMIT ?3"
        );
        let rows: Vec<String> = self.db.with(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let values = statement
                .query_map(params![session_id, since_ms, limit as i64], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(values)
        })?;
        Ok(rows
            .into_iter()
            .filter_map(|body| serde_json::from_str(&body).ok())
            .collect())
    }

    pub fn all_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        self.list_messages(session_id, 1_000_000, "asc", None)
    }

    pub fn count_messages(&self, session_id: &str) -> Result<u64> {
        let count: i64 = self.db.with(|connection| {
            connection.query_row(
                "SELECT COUNT(1) FROM messages WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
        })?;
        Ok(count as u64)
    }

    pub fn last_user_text(&self, session_id: &str) -> Result<Option<String>> {
        let body: Option<String> = self.db.with(|connection| {
            connection
                .query_row(
                    "SELECT data_json FROM messages WHERE session_id = ?1 AND data_json LIKE '%\"type\":\"user\"%'
                     ORDER BY seq DESC LIMIT 1",
                    params![session_id],
                    |row| row.get(0),
                )
                .optional()
        })?;
        Ok(body.and_then(|body| {
            serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| value.get("text").and_then(Value::as_str).map(str::to_string))
        }))
    }

    pub fn create_permission(&self, permission: &PermissionInfo) -> Result<()> {
        let body = serde_json::to_string(permission)?;
        self.db.with(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO permissions (id, session_id, time_created, decision, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    permission.id,
                    permission.session_id,
                    ids::now_ms(),
                    permission.decision,
                    body
                ],
            )
        })?;
        Ok(())
    }

    pub fn list_permissions(&self, session_id: &str) -> Result<Vec<PermissionInfo>> {
        let rows: Vec<String> = self.db.with(|connection| {
            let mut statement = connection.prepare(
                "SELECT data_json FROM permissions WHERE session_id = ?1 AND decision IS NULL
                 ORDER BY time_created ASC",
            )?;
            let values = statement
                .query_map(params![session_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(values)
        })?;
        Ok(rows
            .into_iter()
            .filter_map(|body| serde_json::from_str(&body).ok())
            .collect())
    }

    pub fn get_permission(&self, session_id: &str, id: &str) -> Result<Option<PermissionInfo>> {
        let body: Option<String> = self.db.with(|connection| {
            connection
                .query_row(
                    "SELECT data_json FROM permissions WHERE session_id = ?1 AND id = ?2",
                    params![session_id, id],
                    |row| row.get(0),
                )
                .optional()
        })?;
        Ok(body.and_then(|body| serde_json::from_str(&body).ok()))
    }

    pub fn reply_permission(
        &self,
        session_id: &str,
        id: &str,
        decision: &str,
    ) -> Result<Option<PermissionInfo>> {
        let mut permission = match self.get_permission(session_id, id)? {
            Some(permission) => permission,
            None => return Ok(None),
        };
        permission.decision = Some(decision.to_string());
        let body = serde_json::to_string(&permission)?;
        self.db.with(|connection| {
            connection.execute(
                "UPDATE permissions SET decision = ?1, data_json = ?2 WHERE id = ?3 AND session_id = ?4",
                params![decision, body, id, session_id],
            )
        })?;
        Ok(Some(permission))
    }

    pub fn replace_todos(&self, session_id: &str, todos: &[Value]) -> Result<()> {
        self.db.with_mut(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute("DELETE FROM todos WHERE session_id = ?1", params![session_id])?;
            for (index, todo) in todos.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO todos (session_id, position, data_json) VALUES (?1, ?2, ?3)",
                    params![session_id, index as i64, serde_json::to_string(todo).unwrap_or_default()],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn list_todos(&self, session_id: &str) -> Result<Vec<Value>> {
        let rows: Vec<String> = self.db.with(|connection| {
            let mut statement =
                connection.prepare("SELECT data_json FROM todos WHERE session_id = ?1 ORDER BY position")?;
            let values = statement
                .query_map(params![session_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(values)
        })?;
        Ok(rows
            .into_iter()
            .filter_map(|body| serde_json::from_str(&body).ok())
            .collect())
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        self.db.with(|connection| {
            connection
                .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
                    row.get(0)
                })
                .optional()
        })
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.db.with(|connection| {
            connection.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
        })?;
        Ok(())
    }

    pub fn touch_session(&self, session_id: &str) -> Result<()> {
        let now = ids::now_ms();
        self.db.with(|connection| {
            connection.execute(
                "UPDATE sessions SET time_updated = ?1, data_json = json_set(data_json, '$.time.updated', ?1) WHERE id = ?2",
                params![now, session_id],
            )
        })?;
        Ok(())
    }

    pub fn set_outcome(&self, session_id: &str, outcome: &str) -> Result<Option<SessionInfo>> {
        let Some(mut session) = self.get_session(session_id)? else {
            return Ok(None);
        };
        session.outcome = Some(outcome.to_string());
        session.time.updated = ids::now_ms();
        if outcome == "succeeded" || outcome == "failed" || outcome == "interrupted" {
            session.time.idle = Some(ids::now_ms());
        }
        let body = serde_json::to_string(&session)?;
        self.db.with(|connection| {
            connection.execute(
                "UPDATE sessions SET time_updated = ?1, data_json = ?2 WHERE id = ?3",
                params![session.time.updated, body, session_id],
            )
        })?;
        Ok(Some(session))
    }

    pub fn session_location(&self, session_id: &str) -> Result<Option<LocationRef>> {
        Ok(self.get_session(session_id)?.map(|session| session.location))
    }

    pub fn database_integrity(&self) -> Result<String> {
        self.db.integrity_check()
    }
}
