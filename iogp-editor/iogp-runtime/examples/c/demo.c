/* cc -O2 demo.c -I../../include -L<target>/release -l:libiogp_runtime.a -lpthread -ldl -lm -o demo
 * ./demo config.json   — a worker on the rig floor loses the harness at t=1.0 s; LSR9 needs 3 s. */
#include "iogp_runtime.h"
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static char *slurp(const char *p) {
    FILE *f = fopen(p, "rb"); if (!f) { perror(p); exit(2); }
    fseek(f, 0, SEEK_END); long n = ftell(f); fseek(f, 0, SEEK_SET);
    char *s = malloc(n + 1); if (fread(s, 1, n, f) != (size_t)n) exit(2); s[n] = 0; fclose(f); return s;
}

int main(int argc, char **argv) {
    char *err = NULL;
    char *cfg = slurp(argc > 1 ? argv[1] : "config.json");
    iogp_engine *e = iogp_engine_new(cfg, &err);
    if (!e) { fprintf(stderr, "config: %s\n", err); iogp_string_free(err); return 1; }
    int raised = 0;
    for (int i = 0; i < 80; i++) {
        double t = i / 10.0;
        iogp_frame_begin(e, t, "CAM-03");
        iogp_frame_object(e, "Rig-Floor-East", IOGP_ZONE, "height-zone", NULL);
        iogp_frame_object(e, "7", IOGP_CLASS, "person", NULL);
        iogp_frame_fact(e, "in", "7", "Rig-Floor-East");
        if (t < 1.0) { iogp_frame_object(e, "h3", IOGP_CLASS, "harness", NULL); iogp_frame_fact(e, "wear", "7", "h3"); }
        int n = iogp_frame_end(e);
        for (int k = 0; k < n; k++) {
            iogp_event ev; iogp_event_get(e, k, &ev);
            if (ev.kind == IOGP_RAISED) raised++;
            printf("t=%.1f %s %s LSR%u %s %s subject=%s\n", ev.t, ev.kind == IOGP_RAISED ? "RAISED " : "cleared", ev.rule, ev.lsr, ev.severity, ev.alert, ev.subject);
        }
    }
    /* timing: 100k frames through the builder */
    struct timespec a, b; clock_gettime(CLOCK_MONOTONIC, &a);
    for (int i = 0; i < 100000; i++) {
        iogp_frame_begin(e, 10.0 + i / 10.0, "CAM-03");
        iogp_frame_object(e, "Rig-Floor-East", IOGP_ZONE, "height-zone", NULL);
        iogp_frame_object(e, "7", IOGP_CLASS, "person", NULL);
        iogp_frame_object(e, "h3", IOGP_CLASS, "harness", NULL);
        iogp_frame_fact(e, "in", "7", "Rig-Floor-East");
        iogp_frame_fact(e, "wear", "7", "h3");
        iogp_frame_end(e);
    }
    clock_gettime(CLOCK_MONOTONIC, &b);
    printf("C ABI: %.2f us per frame (1 worker, all rules in the config)\n", ((b.tv_sec - a.tv_sec) * 1e9 + (b.tv_nsec - a.tv_nsec)) / 1e3 / 100000);
    const char *js = iogp_step_json(e, "{\"t\": 20000.0, \"objects\": [{\"id\": \"p\", \"class\": \"person\"}], \"facts\": []}");
    printf("json step: %s\n", js ? js : iogp_last_error(e));
    iogp_engine_free(e); free(cfg);
    return raised >= 1 ? 0 : 3;   /* the working-at-height rule must fire */
}
