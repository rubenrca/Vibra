#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define VIBRA_LISTEN_ADDR_LEN 48
#define VIBRA_LISTEN_NAME_LEN 64
#define VIBRA_LISTEN_CMD_LEN 160

typedef struct {
    uint32_t pid;
    uint16_t port;
    uint8_t ipv6;
    uint8_t _pad;
    char address[VIBRA_LISTEN_ADDR_LEN];
    char name[VIBRA_LISTEN_NAME_LEN];
    char command[VIBRA_LISTEN_CMD_LEN];
} VibraListenSocket;

/// Listening TCP sockets in `root_pid`'s process tree (and same controlling TTY).
/// Returns the number of entries written, or 0 on failure.
int vibra_scan_listen_sockets(uint32_t root_pid, VibraListenSocket *out, int capacity);

/// Listening TCP sockets of a single process (no tree walk).
int vibra_scan_pid_listen_sockets(uint32_t pid, VibraListenSocket *out, int capacity);

/// PIDs owned by the current user. Returns the number written.
int vibra_list_user_pids(uint32_t *out, int capacity);

/// Working directory of `pid`. Returns 1 on success.
int vibra_pid_cwd(uint32_t pid, char *out, int capacity);

#ifdef __cplusplus
}
#endif
