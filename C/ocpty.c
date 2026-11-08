#define _GNU_SOURCE

#include "ocpty.h"

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

extern char **environ;

static int set_winsize(int fd, unsigned short rows, unsigned short cols) {
  struct winsize ws;
  memset(&ws, 0, sizeof(ws));
  ws.ws_row = rows ? rows : 24;
  ws.ws_col = cols ? cols : 80;
  if (ioctl(fd, TIOCSWINSZ, &ws) != 0) return -errno;
  return 0;
}

int oc_pty_open(int *master_fd, char *slave_name, size_t slave_name_len,
                unsigned short rows, unsigned short cols) {
  if (!master_fd || !slave_name || slave_name_len < 2) return -EINVAL;

  int master = posix_openpt(O_RDWR | O_NOCTTY);
  if (master < 0) return -errno;
  if (grantpt(master) != 0) {
    int err = errno;
    close(master);
    return -err;
  }
  if (unlockpt(master) != 0) {
    int err = errno;
    close(master);
    return -err;
  }

  const char *name = ptsname(master);
  if (!name) {
    int err = errno ? errno : EIO;
    close(master);
    return -err;
  }
  size_t len = strlen(name);
  if (len + 1 > slave_name_len) {
    close(master);
    return -ENAMETOOLONG;
  }
  memcpy(slave_name, name, len + 1);

  int rc = set_winsize(master, rows, cols);
  if (rc != 0) {
    close(master);
    return rc;
  }

  *master_fd = master;
  return 0;
}

int oc_pty_spawn(const char *cwd, const char *const argv[],
                 const char *const envp[], unsigned short rows,
                 unsigned short cols, int *master_fd, pid_t *pid_out) {
  if (!argv || !argv[0] || !master_fd || !pid_out) return -EINVAL;

  char name[256];
  int master = -1;
  int rc = oc_pty_open(&master, name, sizeof(name), rows, cols);
  if (rc != 0) return rc;

  int slave = open(name, O_RDWR | O_NOCTTY);
  if (slave < 0) {
    int err = errno;
    close(master);
    return -err;
  }

  pid_t pid = fork();
  if (pid < 0) {
    int err = errno;
    close(slave);
    close(master);
    return -err;
  }

  if (pid == 0) {
    /* Child: become a session leader and attach the PTY as the controlling
     * terminal so job control and Ctrl+C behave exactly like a real shell. */
    setsid();
    ioctl(slave, TIOCSCTTY, 0);
    dup2(slave, STDIN_FILENO);
    dup2(slave, STDOUT_FILENO);
    dup2(slave, STDERR_FILENO);
    if (slave > STDERR_FILENO) close(slave);
    close(master);

    signal(SIGPIPE, SIG_DFL);
    signal(SIGINT, SIG_DFL);
    signal(SIGQUIT, SIG_DFL);
    signal(SIGTSTP, SIG_DFL);

    if (cwd && *cwd && chdir(cwd) != 0) _exit(127);
    if (envp) environ = (char **)envp;
    execvp(argv[0], (char *const *)argv);
    _exit(127);
  }

  close(slave);
  oc_pty_nonblock(master);
  *master_fd = master;
  *pid_out = pid;
  return 0;
}

int oc_pty_resize(int fd, unsigned short rows, unsigned short cols) {
  return set_winsize(fd, rows, cols);
}

int oc_pty_nonblock(int fd) {
  int flags = fcntl(fd, F_GETFL, 0);
  if (flags < 0) return -errno;
  if (fcntl(fd, F_SETFL, flags | O_NONBLOCK) != 0) return -errno;
  return 0;
}

pid_t oc_pty_wait(pid_t pid, int *status, int nohang) {
  int local = 0;
  pid_t rc = waitpid(pid, status ? status : &local, nohang ? WNOHANG : 0);
  if (rc < 0) return -errno;
  return rc;
}

int oc_pty_terminate(pid_t pid) {
  if (pid <= 0) return -EINVAL;
  if (kill(pid, SIGTERM) != 0 && errno != ESRCH) return -errno;
  int status = 0;
  for (int i = 0; i < 50; i++) {
    pid_t rc = waitpid(pid, &status, WNOHANG);
    if (rc == pid) return 0;
    if (rc < 0 && errno != EINTR) return -errno;
    struct timespec ts;
    ts.tv_sec = 0;
    ts.tv_nsec = 20 * 1000 * 1000;
    nanosleep(&ts, NULL);
  }
  kill(pid, SIGKILL);
  waitpid(pid, &status, 0);
  return 0;
}

int oc_term_size(int fd, unsigned short *rows, unsigned short *cols) {
  struct winsize ws;
  if (ioctl(fd, TIOCGWINSZ, &ws) != 0) return -errno;
  if (rows) *rows = ws.ws_row;
  if (cols) *cols = ws.ws_col;
  return 0;
}

int oc_fsync_dir(const char *path) {
  if (!path) return -EINVAL;
  char *copy = strdup(path);
  if (!copy) return -ENOMEM;
  char *slash = strrchr(copy, '/');
  const char *dir = ".";
  if (slash) {
    if (slash == copy)
      slash[1] = '\0';
    else
      *slash = '\0';
    dir = copy;
  }
  int fd = open(dir, O_RDONLY | O_DIRECTORY);
  int rc = 0;
  if (fd < 0) {
    rc = -errno;
  } else {
    if (fsync(fd) != 0) rc = -errno;
    close(fd);
  }
  free(copy);
  return rc;
}

int oc_statvfs(const char *path, unsigned long long *available,
               unsigned long long *total) {
  struct statvfs vfs;
  if (!path) return -EINVAL;
  if (statvfs(path, &vfs) != 0) return -errno;
  if (available) *available = (unsigned long long)vfs.f_bavail * vfs.f_frsize;
  if (total) *total = (unsigned long long)vfs.f_blocks * vfs.f_frsize;
  return 0;
}

/* Literal search. mmap keeps memory bounded and lets us scan at memory speed;
 * counting newlines only up to the match keeps the hot loop branch-light. */
long oc_search_file(const char *path, const char *needle, size_t needle_len,
                    oc_match *out, size_t out_cap) {
  if (!path || !needle || needle_len == 0 || !out || out_cap == 0)
    return -EINVAL;

  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0) return -errno;

  struct stat st;
  if (fstat(fd, &st) != 0) {
    int err = errno;
    close(fd);
    return -err;
  }
  if (st.st_size <= 0 || !S_ISREG(st.st_mode)) {
    close(fd);
    return 0;
  }
  if ((unsigned long long)st.st_size > (256ull * 1024ull * 1024ull)) {
    close(fd);
    return -EFBIG;
  }

  const unsigned char *base =
      (const unsigned char *)mmap(NULL, (size_t)st.st_size, PROT_READ,
                                  MAP_PRIVATE, fd, 0);
  close(fd);
  if (base == MAP_FAILED) return -errno;

  size_t size = (size_t)st.st_size;
  size_t pos = 0;
  size_t count = 0;
  unsigned int line = 1;
  size_t line_start = 0;
  const unsigned char first = (const unsigned char)needle[0];
  const unsigned char *needle_bytes = (const unsigned char *)needle;

  while (pos + needle_len <= size && count < out_cap) {
    const unsigned char *hit =
        (const unsigned char *)memchr(base + pos, first, size - pos - needle_len + 1);
    if (!hit) break;
    size_t at = (size_t)(hit - base);
    if (memcmp(hit, needle_bytes, needle_len) == 0) {
      for (size_t i = line_start; i < at; i++) {
        if (base[i] == '\n') {
          line++;
          line_start = i + 1;
        }
      }
      out[count].offset = (unsigned long long)at;
      out[count].line = line;
      out[count].column = (unsigned int)(at - line_start + 1);
      count++;
      pos = at + 1;
    } else {
      pos = at + 1;
    }
  }

  munmap((void *)base, size);
  return (long)count;
}
