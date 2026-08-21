#include "process_inspect.h"

#include <arpa/inet.h>
#include <libproc.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/sysctl.h>
#include <sys/types.h>
#include <unistd.h>

#define MAX_TREE_PIDS 1024
#define MAX_QUEUE 1024
#define MAX_CHILDREN 256
#define MAX_TTY_PIDS 512

static int contains_pid(const uint32_t *pids, int n, uint32_t pid)
{
    for (int i = 0; i < n; i++) {
        if (pids[i] == pid) {
            return 1;
        }
    }
    return 0;
}

static void copy_cstr(char *dst, size_t dst_len, const char *src)
{
    if (dst_len == 0) {
        return;
    }
    if (src == NULL) {
        dst[0] = '\0';
        return;
    }
    strncpy(dst, src, dst_len - 1);
    dst[dst_len - 1] = '\0';
}

static void basename_of(const char *path, char *out, size_t out_len)
{
    if (path == NULL || path[0] == '\0') {
        copy_cstr(out, out_len, "");
        return;
    }
    const char *slash = strrchr(path, '/');
    copy_cstr(out, out_len, slash != NULL ? slash + 1 : path);
}

static void fill_process_name(uint32_t pid, char *out, size_t out_len)
{
    char path[PROC_PIDPATHINFO_MAXSIZE];
    memset(path, 0, sizeof(path));
    int n = proc_pidpath((int)pid, path, sizeof(path));
    if (n > 0) {
        basename_of(path, out, out_len);
        if (out[0] != '\0') {
            return;
        }
    }
    char name[VIBRA_LISTEN_NAME_LEN];
    memset(name, 0, sizeof(name));
    if (proc_name((int)pid, name, sizeof(name)) > 0) {
        copy_cstr(out, out_len, name);
        return;
    }
    out[0] = '\0';
}

static void fill_command_line(uint32_t pid, char *out, size_t out_len)
{
    out[0] = '\0';
    int mib[3] = {CTL_KERN, KERN_PROCARGS2, (int)pid};
    size_t size = 0;
    if (sysctl(mib, 3, NULL, &size, NULL, 0) != 0 || size < sizeof(int) + 2) {
        return;
    }
    if (size > 32 * 1024) {
        size = 32 * 1024;
    }
    char *buf = malloc(size);
    if (buf == NULL) {
        return;
    }
    if (sysctl(mib, 3, buf, &size, NULL, 0) != 0 || size < sizeof(int) + 2) {
        free(buf);
        return;
    }
    int argc = 0;
    memcpy(&argc, buf, sizeof(argc));
    if (argc < 0 || argc > 256) {
        free(buf);
        return;
    }
    char *end = buf + size;
    char *cursor = buf + sizeof(int);
    while (cursor < end && *cursor != '\0') {
        cursor++;
    }
    if (cursor >= end) {
        free(buf);
        return;
    }
    cursor++;
    while (cursor < end && *cursor == '\0') {
        cursor++;
    }

    size_t used = 0;
    for (int i = 0; i < argc && cursor < end && used + 1 < out_len; i++) {
        if (*cursor == '\0') {
            break;
        }
        if (used > 0) {
            out[used++] = ' ';
        }
        while (cursor < end && *cursor != '\0' && used + 1 < out_len) {
            out[used++] = *cursor++;
        }
        while (cursor < end && *cursor != '\0') {
            cursor++;
        }
        if (cursor < end && *cursor == '\0') {
            cursor++;
        }
    }
    out[used] = '\0';
    free(buf);
}

static int collect_tree(uint32_t root, uint32_t *out, int cap)
{
    if (cap <= 0 || root == 0) {
        return 0;
    }
    uint32_t queue[MAX_QUEUE];
    int qh = 0;
    int qt = 0;
    int n = 0;
    queue[qt++] = root;
    out[n++] = root;

    while (qh < qt) {
        uint32_t pid = queue[qh++];
        int children[MAX_CHILDREN];
        int bytes = proc_listchildpids((pid_t)pid, children, (int)sizeof(children));
        if (bytes <= 0) {
            continue;
        }
        int count = bytes / (int)sizeof(int);
        if (count > MAX_CHILDREN) {
            count = MAX_CHILDREN;
        }
        for (int i = 0; i < count; i++) {
            uint32_t child = (uint32_t)children[i];
            if (child == 0 || child == pid || contains_pid(out, n, child)) {
                continue;
            }
            if (n < cap) {
                out[n++] = child;
            }
            if (qt < MAX_QUEUE) {
                queue[qt++] = child;
            }
        }
    }

    struct proc_bsdinfo bsd;
    memset(&bsd, 0, sizeof(bsd));
    int got = proc_pidinfo((int)root, PROC_PIDTBSDINFO, 0, &bsd, (int)sizeof(bsd));
    if (got == (int)sizeof(bsd) && bsd.e_tdev != 0) {
        int tty_pids[MAX_TTY_PIDS];
        int tbytes = proc_listpids(PROC_TTY_ONLY, bsd.e_tdev, tty_pids, (int)sizeof(tty_pids));
        if (tbytes > 0) {
            int tcount = tbytes / (int)sizeof(int);
            if (tcount > MAX_TTY_PIDS) {
                tcount = MAX_TTY_PIDS;
            }
            for (int i = 0; i < tcount; i++) {
                uint32_t pid = (uint32_t)tty_pids[i];
                if (pid == 0 || contains_pid(out, n, pid)) {
                    continue;
                }
                if (n < cap) {
                    out[n++] = pid;
                }
            }
        }
    }
    return n;
}

static void format_listen_address(const struct in_sockinfo *info, char *out, size_t out_len, uint8_t *ipv6)
{
    *ipv6 = 0;
    out[0] = '\0';
    if (info->insi_vflag & INI_IPV4) {
        inet_ntop(AF_INET, &info->insi_laddr.ina_46.i46a_addr4, out, (socklen_t)out_len);
        return;
    }
    if (info->insi_vflag & INI_IPV6) {
        const struct in6_addr *addr = &info->insi_laddr.ina_6;
        if (IN6_IS_ADDR_V4MAPPED(addr)) {
            struct in_addr v4;
            memcpy(&v4, &addr->s6_addr[12], sizeof(v4));
            inet_ntop(AF_INET, &v4, out, (socklen_t)out_len);
            return;
        }
        *ipv6 = 1;
        inet_ntop(AF_INET6, addr, out, (socklen_t)out_len);
        return;
    }
    copy_cstr(out, out_len, "0.0.0.0");
}

static int append_listen_sockets(uint32_t pid, VibraListenSocket *out, int capacity, int filled)
{
    if (filled >= capacity) {
        return filled;
    }
    int needed = proc_pidinfo((int)pid, PROC_PIDLISTFDS, 0, NULL, 0);
    if (needed <= 0) {
        return filled;
    }
    struct proc_fdinfo *fds = malloc((size_t)needed);
    if (fds == NULL) {
        return filled;
    }
    int bytes = proc_pidinfo((int)pid, PROC_PIDLISTFDS, 0, fds, needed);
    if (bytes <= 0) {
        free(fds);
        return filled;
    }
    int nfd = bytes / (int)sizeof(struct proc_fdinfo);
    char name[VIBRA_LISTEN_NAME_LEN];
    char command[VIBRA_LISTEN_CMD_LEN];
    int named = 0;
    for (int i = 0; i < nfd && filled < capacity; i++) {
        if (fds[i].proc_fdtype != PROX_FDTYPE_SOCKET) {
            continue;
        }
        struct socket_fdinfo socket;
        memset(&socket, 0, sizeof(socket));
        int got = proc_pidfdinfo(
            (int)pid,
            fds[i].proc_fd,
            PROC_PIDFDSOCKETINFO,
            &socket,
            (int)sizeof(socket)
        );
        if (got < (int)sizeof(socket)) {
            continue;
        }
        if (socket.psi.soi_kind != SOCKINFO_TCP) {
            continue;
        }
        if (socket.psi.soi_proto.pri_tcp.tcpsi_state != TSI_S_LISTEN) {
            continue;
        }
        uint16_t port = ntohs((uint16_t)socket.psi.soi_proto.pri_tcp.tcpsi_ini.insi_lport);
        if (port == 0) {
            continue;
        }
        if (!named) {
            fill_process_name(pid, name, sizeof(name));
            fill_command_line(pid, command, sizeof(command));
            named = 1;
        }
        VibraListenSocket *row = &out[filled];
        memset(row, 0, sizeof(*row));
        row->pid = pid;
        row->port = port;
        format_listen_address(
            &socket.psi.soi_proto.pri_tcp.tcpsi_ini,
            row->address,
            sizeof(row->address),
            &row->ipv6
        );
        copy_cstr(row->name, sizeof(row->name), name);
        copy_cstr(row->command, sizeof(row->command), command);
        filled++;
    }
    free(fds);
    return filled;
}

int vibra_scan_listen_sockets(uint32_t root_pid, VibraListenSocket *out, int capacity)
{
    if (out == NULL || capacity <= 0 || root_pid == 0) {
        return 0;
    }
    uint32_t pids[MAX_TREE_PIDS];
    int n = collect_tree(root_pid, pids, MAX_TREE_PIDS);
    int filled = 0;
    for (int i = 0; i < n && filled < capacity; i++) {
        filled = append_listen_sockets(pids[i], out, capacity, filled);
    }
    return filled;
}

int vibra_scan_pid_listen_sockets(uint32_t pid, VibraListenSocket *out, int capacity)
{
    if (out == NULL || capacity <= 0 || pid == 0) {
        return 0;
    }
    return append_listen_sockets(pid, out, capacity, 0);
}

int vibra_list_user_pids(uint32_t *out, int capacity)
{
    if (out == NULL || capacity <= 0) {
        return 0;
    }
    /* `proc_listallpids(NULL, 0)` is either a byte count or a pid count
       depending on OS rev; allocate a large fixed buffer instead. */
    enum { ALL_CAP = 8192 };
    int *all = malloc(sizeof(int) * ALL_CAP);
    if (all == NULL) {
        return 0;
    }
    int bytes = proc_listallpids(all, (int)(sizeof(int) * ALL_CAP));
    if (bytes <= 0) {
        free(all);
        return 0;
    }
    int count = bytes / (int)sizeof(int);
    if (count > ALL_CAP) {
        count = ALL_CAP;
    }
    uid_t uid = getuid();
    int filled = 0;
    for (int i = 0; i < count && filled < capacity; i++) {
        uint32_t pid = (uint32_t)all[i];
        if (pid <= 1) {
            continue;
        }
        struct proc_bsdshortinfo info;
        memset(&info, 0, sizeof(info));
        int got = proc_pidinfo(
            (int)pid,
            PROC_PIDT_SHORTBSDINFO,
            0,
            &info,
            (int)sizeof(info)
        );
        if (got == (int)sizeof(info) && info.pbsi_uid != uid) {
            continue;
        }
        out[filled++] = pid;
    }
    free(all);
    return filled;
}

int vibra_pid_cwd(uint32_t pid, char *out, int capacity)
{
    if (out == NULL || capacity <= 0 || pid == 0) {
        return 0;
    }
    struct proc_vnodepathinfo info;
    memset(&info, 0, sizeof(info));
    int got = proc_pidinfo(
        (int)pid,
        PROC_PIDVNODEPATHINFO,
        0,
        &info,
        (int)sizeof(info)
    );
    if (got != (int)sizeof(info) || info.pvi_cdir.vip_path[0] == '\0') {
        return 0;
    }
    copy_cstr(out, (size_t)capacity, info.pvi_cdir.vip_path);
    return 1;
}
