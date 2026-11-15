/* iogp_runtime.h — inference-time evaluator for IOGP alert rules (config.json from iogp-editor).
 *
 * Link with libiogp_runtime.a (static) or libiogp_runtime.so (dynamic); on Linux add -lpthread -ldl -lm.
 * One engine per camera stream. An engine may move between threads but must not be used by two
 * threads at once. All strings are UTF-8 and NUL-terminated.
 *
 *   iogp_engine *e = iogp_engine_new(config_json_text, &err);
 *   for each frame:
 *     iogp_frame_begin(e, t_seconds, "CAM-03");
 *     iogp_frame_object(e, "7",  IOGP_CLASS, "person", NULL);
 *     iogp_frame_object(e, "z1", IOGP_ZONE,  "height-zone", NULL);
 *     iogp_frame_fact(e, "in", "7", "z1");
 *     int n = iogp_frame_end(e);
 *     for (int i = 0; i < n; i++) { iogp_event ev; iogp_event_get(e, i, &ev); ... }
 *   iogp_engine_free(e);
 *
 * Event strings stay valid until the next iogp_frame_end / iogp_step_json on the same engine.
 */
#ifndef IOGP_RUNTIME_H
#define IOGP_RUNTIME_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct IogpEngine iogp_engine;

enum { IOGP_CLASS = 0, IOGP_ZONE = 1, IOGP_SIGNAL = 2 };
enum { IOGP_RAISED = 1, IOGP_CLEARED = 2 };

typedef struct {
    int kind;              /* IOGP_RAISED or IOGP_CLEARED */
    const char *rule;      /* rule id, e.g. "LSR9" */
    const char *alert;     /* e.g. "working-at-height-alert" */
    uint8_t lsr;           /* IOGP Life-Saving Rule number 1..9 */
    const char *severity;  /* "critical" | "high" | "medium" | "low" | "" */
    const char *subject;   /* object id (e.g. track id), zone id, or "scene" */
    double t;              /* frame time */
    const char *proof;     /* why: rule + conditions that held, '\n'-separated ("" when cleared) */
} iogp_event;

/* Create from config.json text. NULL on failure; *err (if err != NULL) then holds a message to free with iogp_string_free. */
iogp_engine *iogp_engine_new(const char *config_json, char **err);
void iogp_engine_free(iogp_engine *e);
void iogp_string_free(char *s);

/* Zero-copy frame builder. camera may be NULL. Return 0 on success, -1 on bad arguments. */
void iogp_frame_begin(iogp_engine *e, double t, const char *camera);
int  iogp_frame_object(iogp_engine *e, const char *id, int kind, const char *type, const char *camera);
int  iogp_frame_fact(iogp_engine *e, const char *rel, const char *a, const char *b);
/* Evaluate the frame: number of events, or -1. */
int  iogp_frame_end(iogp_engine *e);
int  iogp_event_get(const iogp_engine *e, size_t i, iogp_event *out);

/* Convenience: frame as JSON {"t":..,"camera":..,"objects":[{"id","class"|"zone"|"signal","camera"?}],"facts":[[rel,a,b]]};
 * returns the events as a JSON array, or NULL (see iogp_last_error). */
const char *iogp_step_json(iogp_engine *e, const char *frame_json);
const char *iogp_last_error(const iogp_engine *e);
size_t iogp_active_count(const iogp_engine *e);

#ifdef __cplusplus
}
#endif
#endif
