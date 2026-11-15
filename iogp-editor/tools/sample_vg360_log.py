"""Write a SYNTHETIC sample of a recorded site, in the raw format `iogp_eval replay` reads:
site.json (cameras, zone polygons, permit timeline), detections.jsonl (tracked boxes per camera
frame, with realistic detection drop-outs), labels.jsonl (the alert intervals a safety officer
would mark while reviewing the footage) and rules.ace (the rules under test).

It is a format example and a pipeline check, not evidence: replace it with a real VisionGuard360
export (same field names) to measure the rules on your site.

    python3 tools/sample_vg360_log.py OUTDIR [--seed 7]
"""
import json
import os
import random
import sys

FPS, SECS = 5, 240
out = sys.argv[1]
seed = int(sys.argv[sys.argv.index("--seed") + 1]) if "--seed" in sys.argv else 7
rnd = random.Random(seed)
os.makedirs(out, exist_ok=True)

site = {
    "cameras": [
        {"name": "CAM-03", "zones": [{"id": "Rig-Floor-East", "type": "height-zone", "polygon": [[100, 400], [900, 400], [900, 700], [100, 700]]}]},
        {"name": "CAM-02", "zones": [{"id": "Moonpool", "type": "restricted-area", "polygon": [[500, 380], [1100, 380], [1100, 700], [500, 700]]}]},
    ],
    "class_map": {"hard hat": "hardhat", "safety harness": "harness", "cell phone": "phone", "load": "suspended_load"},
    "signals": [{"subject": "Moonpool", "signal": "permit", "id": "PTW-77", "from": 0, "to": 100}],
}
rules = """WAH3: If a worker is in a height-zone of CAM-03 and does not wear a harness for at least 3 seconds then VisionGuard raises a critical working-at-height-alert for the worker.
LOF: If a worker is under a suspended-load for at least 2 seconds then VisionGuard raises a critical line-of-fire-alert for the worker.
MOON: If a worker is in Moonpool for at least 5 seconds then VisionGuard raises a high work-authorisation-alert for the worker unless Moonpool has a permit.
"""


def person_box(x, inside):
    y2 = 600 if inside else 330          # foot point inside / outside the zone polygon
    return [x, y2 - 170, x + 60, y2]


def miss(p):
    return rnd.random() < p


frames = []
for i in range(SECS * FPS):
    t = round(i / FPS, 2)
    # ---- CAM-03: rig floor. w1 takes the harness off 60-90 s while on the floor; w2's harness is
    # occluded for 1 s at 120 s (shorter than the 3 s rule: must NOT alert); a load swings over w2 150-156 s.
    dets = []
    for tid, x, inside, harness in [
        (1, 200 + 3 * (t % 40), 20 <= t <= 200, not (60 <= t < 90)),
        (2, 600, 10 <= t <= 230, not (120 <= t < 121)),
    ]:
        if inside or t < 230:
            b = person_box(x, inside)
            if not miss(0.03):
                dets.append({"track_id": tid, "class": "person", "bbox": b, "score": 0.9})
            if harness and not miss(0.20):  # harness boxes are small and often missed
                dets.append({"track_id": 100 + tid, "class": "safety harness", "bbox": [b[0] + 10, b[1] + 40, b[0] + 50, b[1] + 100], "score": 0.7})
    if 150 <= t < 156 and not miss(0.1):
        dets.append({"track_id": 50, "class": "load", "bbox": [560, 200, 720, 300], "score": 0.8})
    frames.append({"t": t, "camera": "CAM-03", "detections": dets})
    # ---- CAM-02: moonpool. w3 is inside 30-50 s (permit valid until 100 s: fine) and 120-150 s (no permit).
    dets = []
    inside = 30 <= t < 50 or 120 <= t < 150
    x = 700 if inside else 200
    b = [x, (600 if inside else 330) - 170, x + 60, 600 if inside else 330]
    if not miss(0.05):
        dets.append({"track_id": 3, "class": "person", "bbox": b, "score": 0.88})
    frames.append({"t": t, "camera": "CAM-02", "detections": dets})

labels = [
    {"rule": "WAH3", "camera": "CAM-03", "start": 60, "end": 90, "subject": "1", "note": "w1 without harness on the rig floor"},
    {"rule": "LOF", "camera": "CAM-03", "start": 150, "end": 156, "subject": "2", "note": "w2 under the swinging load"},
    {"rule": "MOON", "camera": "CAM-02", "start": 120, "end": 150, "subject": "3", "note": "w3 in the moonpool after the permit expired"},
]
json.dump(site, open(f"{out}/site.json", "w"), indent=1)
with open(f"{out}/detections.jsonl", "w") as f:
    for fr in frames:
        f.write(json.dumps(fr) + "\n")
with open(f"{out}/labels.jsonl", "w") as f:
    for l in labels:
        f.write(json.dumps(l) + "\n")
open(f"{out}/rules.ace", "w").write(rules)
print(f"{len(frames)} camera frames, {len(labels)} labelled incidents -> {out}/")
