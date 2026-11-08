/*
 * OxideCode native runtime (C)
 *
 * Thin, allocation-free primitives that sit below the Rust layers:
 *   - PTY allocation, spawn, resize and wait (Termux / Android bionic)
 *   - literal file search over mmap with line/column reporting
 *   - durable-fsync helpers for atomic writes
 *   - filesystem capacity queries
 *
 * Everything here is deliberately plain C11 with no libc extensions that
 * bionic does not provide (no forkpty, no memmem requirement).
 */
#ifndef OXIDECODE_OCPTY_H
#define OXIDECODE_OCPTY_H

#include <stddef.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct oc_match {
  unsigned long long offset; /* byte offset in file */
  unsigned int line;         /* 1-based line number */
  unsigned int column;       /* 1-based byte column */
} oc_match;

/* Opens a new PTY master. Writes the slave device path into slave_name.
 * Returns 0 on success, negative errno on failure. */
int oc_pty_open(int *master_fd, char *slave_name, size_t slave_name_len,
                unsigned short rows, unsigned short cols);

/* Forks a child attached to a fresh PTY and execs argv[0] (PATH searched).
 * cwd may be NULL. envp may be NULL to inherit the environment.
 * Returns 0 on success, negative errno on failure. */
int oc_pty_spawn(const char *cwd, const char *const argv[],
                 const char *const envp[], unsigned short rows,
                 unsigned short cols, int *master_fd, pid_t *pid_out);

/* Sets the PTY window size. Returns 0 or negative errno. */
int oc_pty_resize(int fd, unsigned short rows, unsigned short cols);

/* Puts a file descriptor in non-blocking mode. Returns 0 or negative errno. */
int oc_pty_nonblock(int fd);

/* Waits for a child. nohang != 0 => WNOHANG. Returns pid, 0 when still
 * running, or negative errno. */
pid_t oc_pty_wait(pid_t pid, int *status, int nohang);

/* Terminates a child with SIGTERM and reaps it. Returns 0 or negative errno. */
int oc_pty_terminate(pid_t pid);

/* Reads the terminal size of fd. Returns 0 or negative errno. */
int oc_term_size(int fd, unsigned short *rows, unsigned short *cols);

/* fsync()s the directory containing path (used after rename). */
int oc_fsync_dir(const char *path);

/* Reports free/total bytes for the filesystem holding path. */
int oc_statvfs(const char *path, unsigned long long *available,
               unsigned long long *total);

/* Searches a file for a literal needle. Fills up to out_cap matches,
 * returns the number written or negative errno. */
long oc_search_file(const char *path, const char *needle, size_t needle_len,
                    oc_match *out, size_t out_cap);

#ifdef __cplusplus
}
#endif

#endif /* OXIDECODE_OCPTY_H */
