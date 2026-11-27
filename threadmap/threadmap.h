/* threadmap.h: C ABI of the Domain OS Toolbox "Thread map" logic (Rust crate threadmap, src/ffi.rs).
 *
 * Rules (PLAN.md "C ABI"): functions are prefixed tm_; strings are UTF-8 and NUL-terminated; strings returned by
 * the library are owned by it and freed with tm_free_str; bigger results are JSON. No panic crosses the boundary:
 * functions return TM_OK / a negative TM_ERR_* code, or NULL / 0 / a negative float on error.
 * A tm_map is internally locked: the ETW thread may feed while the UI thread reads. Feed in batches.
 *
 * Timestamps are raw QPC ticks (EVENT_RECORD.EventHeader.TimeStamp of a QPC-clock session); the frequency is given
 * once to tm_new.
 */
#ifndef THREADMAP_H
#define THREADMAP_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TM_OK 0
#define TM_ERR_NULL (-1)     /* NULL handle */
#define TM_ERR_ARG (-2)      /* bad argument (NULL array with count > 0, unknown node id) */
#define TM_ERR_INTERNAL (-3) /* internal error (caught panic) */

/* KTHREAD_STATE values used in tm_cswitch.old_state. */
#define TM_STATE_READY 1
#define TM_STATE_RUNNING 2
#define TM_STATE_STANDBY 3
#define TM_STATE_TERMINATED 4
#define TM_STATE_WAITING 5
#define TM_STATE_DEFERRED_READY 7

#define TM_READIER_UNKNOWN 0xFFFFFFFFu /* tm_ready.readier_tid: use the thread running on that cpu */
#define TM_READY_FLAG_DPC 0x1          /* ReadyThread Flag: readied from a DPC */

/* Link kinds (tm_link.kind) and their sources (tm_link.via). */
#define TM_LINK_WAKE 0
#define TM_LINK_PREEMPT 1
#define TM_LINK_LOCK 2
#define TM_LINK_IPC 3
#define TM_VIA_SCHED 0
#define TM_VIA_LPC_WAIT 1
#define TM_VIA_ALPC 2
#define TM_VIA_PIPE 3
#define TM_VIA_FILE 4
#define TM_VIA_OTHER 5

/* Node id kinds: id >> 48 == 1 process, 2 thread, 3 merged group. Ids stay below 2^53. */
#define TM_ID_PROCESS (1ull << 48)
#define TM_ID_THREAD (2ull << 48)
#define TM_ID_GROUP (3ull << 48)

typedef struct tm_map tm_map;

/* Thread/CSwitch (opcode 36). 40 bytes. */
typedef struct tm_cswitch {
    int64_t ts;
    uint32_t cpu;
    uint32_t new_tid;
    uint32_t old_tid;
    int8_t new_priority;
    int8_t old_priority;
    uint8_t old_wait_reason; /* KWAIT_REASON */
    uint8_t old_wait_mode;   /* 0 kernel, 1 user */
    uint8_t old_state;       /* KTHREAD_STATE */
    uint8_t reserved[3];
    uint32_t new_wait_time;
    uint32_t reserved2;
} tm_cswitch;

/* Thread/ReadyThread (opcode 50). 24 bytes. */
typedef struct tm_ready {
    int64_t ts;
    uint32_t cpu;
    uint32_t readier_tid; /* header ThreadId if trusted, else TM_READIER_UNKNOWN; 0 = idle (kernel work) */
    uint32_t target_tid;  /* TThreadId */
    int8_t adjust_reason;
    int8_t adjust_increment;
    uint8_t flags;        /* Flag: TM_READY_FLAG_DPC ... */
    uint8_t reserved;
} tm_ready;

/* A link another source already resolved to two threads (ALPC client -> server, pipe, file). 24 bytes. */
typedef struct tm_link {
    int64_t ts;
    uint32_t from_tid;
    uint32_t to_tid;
    uint8_t kind; /* TM_LINK_* (normally TM_LINK_IPC) */
    uint8_t via;  /* TM_VIA_* */
    uint8_t reserved[2];
    uint32_t count; /* messages this event stands for; 0 counts as 1 */
} tm_link;

/* Layout position of one node, in layout space (the UI applies its own pan/zoom). 24 bytes. */
typedef struct tm_pos {
    uint64_t id;
    float x;
    float y;
    float r;         /* radius, same as the snapshot's "r" */
    uint32_t pinned; /* 1 = pinned by a drag */
} tm_pos;

#if defined(__cplusplus)
static_assert(sizeof(tm_cswitch) == 40 && sizeof(tm_ready) == 24 && sizeof(tm_link) == 24 && sizeof(tm_pos) == 24,
              "threadmap.h layout");
#else
_Static_assert(sizeof(tm_cswitch) == 40 && sizeof(tm_ready) == 24 && sizeof(tm_link) == 24 && sizeof(tm_pos) == 24,
               "threadmap.h layout");
#endif

/* ---- life cycle ---------------------------------------------------------------------------------------------- */
const char *tm_version(void);          /* static, do not free */
tm_map *tm_new(uint64_t qpc_freq);     /* NULL when qpc_freq == 0 */
void tm_free(tm_map *m);
void tm_free_str(char *s);             /* strings returned below; NULL is fine */
int32_t tm_clear(tm_map *m);           /* drop all events and history */
int32_t tm_set_game(tm_map *m, uint32_t pid);       /* 0 = none */
int32_t tm_set_cpu_count(tm_map *m, uint32_t n);    /* 0 = highest cpu seen + 1 */
int32_t tm_set_reorder_ms(tm_map *m, uint32_t ms);  /* default 250, max 10000 */

/* ---- feeding (any order across cpus inside the reorder window) ----------------------------------------------- */
int32_t tm_feed_cswitch(tm_map *m, const tm_cswitch *ev, size_t n);
int32_t tm_feed_ready(tm_map *m, const tm_ready *ev, size_t n);
int32_t tm_feed_link(tm_map *m, const tm_link *ev, size_t n);
int32_t tm_process_start(tm_map *m, int64_t ts, uint32_t pid, const char *image);
int32_t tm_process_end(tm_map *m, int64_t ts, uint32_t pid);
int32_t tm_thread_start(tm_map *m, int64_t ts, uint32_t tid, uint32_t pid, const char *description /* or NULL */,
                        const char *start_module /* or NULL */, uint64_t start_offset);
int32_t tm_thread_end(tm_map *m, int64_t ts, uint32_t tid);
int32_t tm_thread_name(tm_map *m, int64_t ts, uint32_t tid, const char *name);
int32_t tm_pump(tm_map *m);  /* process events older than the reorder window (snapshot does it too) */
int32_t tm_flush(tm_map *m); /* process everything held */

/* ---- reading --------------------------------------------------------------------------------------------------
 * tm_snapshot_json(m, query): query is a JSON object, every field optional (NULL = the last query):
 *   end_age (0 = now/live; N = window ends N buckets of 100 ms ago), span (buckets, 1..300, default 10),
 *   focus_pid, neighbours (true), min_rate (links/s, 1.0), hide_system (true), system_names ([...] replaces the
 *   default list), top_n (6, 0 = all), merge (true), expand ([pid...]), unmerge ([group id...]),
 *   kinds (bit mask 1<<TM_LINK_*, 15), max_nodes (300, max 600), min_cpu (0.01 cores).
 * Result: {"window":{from_age,to_age,buckets,seconds,live}, "game_pid", "cpu_count",
 *   "nodes":[{id, kind:"process"|"thread"|"group", class:"game"|"background"|"system"|"kernel"|"unknown", pid, tid?,
 *            parent?, label, image, count, threads_alive?, expanded, cpu (cores), state:"running"|"ready"|
 *            "wait_lock"|"wait_ipc"|"waiting"|"sleeping"|"gone"|"unknown", wait_reason?, wait_reason_name?,
 *            ready_for_us?, wait_p99_us?, waits, wait_ms, wakes_in, wakes_out (per s), prio?, core? (last cpu of a
 *            single thread), running, ready, r,
 *            members?:[{id, tid, label}]}],
 *   "links":[{from, to, kind:"wake"|"preempt"|"lock"|"ipc", rate (per s), count, via?:["alpc",...]}],
 *   "cut":{nodes, links_min_rate, links_top_n, links_hidden, links_cut_nodes, max_nodes}, "stats":{...}}
 * Nodes come in draw order: an expanded process (expanded:true, drawn as a halo) right before its threads.
 * The snapshot also replaces the layout's nodes (positions kept by id).
 */
char *tm_snapshot_json(tm_map *m, const char *query_json);
/* {"bucket_ms":100,"cpu_count","game_pid","load":[300 cores],"wait_ms":[300],"p99_us":[300, -1 = none],
 *  "waits":[300],"valid":[300 0/1]}, oldest first, index 299 = now. */
char *tm_series_json(tm_map *m);
char *tm_stats_json(tm_map *m);

/* ---- layout (deterministic force layout; the UI only draws) --------------------------------------------------- */
float tm_layout_step(tm_map *m, float dt);       /* returns kinetic energy; stop animating below ~0.05 per node */
float tm_layout_warm(tm_map *m, uint32_t steps); /* first placement, e.g. 260 steps */
size_t tm_layout_count(tm_map *m);
size_t tm_layout_positions(tm_map *m, tm_pos *out, size_t cap); /* snapshot node order */
int32_t tm_layout_drag(tm_map *m, uint64_t id, float x, float y); /* move + pin */
int32_t tm_layout_pin(tm_map *m, uint64_t id, bool pinned);
int32_t tm_layout_relayout(tm_map *m); /* unpin all, new spots */

#ifdef __cplusplus
}
#endif
#endif /* THREADMAP_H */
