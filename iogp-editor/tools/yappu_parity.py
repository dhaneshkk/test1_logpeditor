"""Run yappu-fsm's vision EventEngine on a seeded synthetic scenario and dump, per frame, the facts
it derived and the alerts it holds active, in iogp-editor's runtime frame format.

    python3 tools/yappu_parity.py /path/to/yappu-fsm out.jsonl

`iogp-eval parity out.jsonl` then replays the facts through iogp-editor's engine (rules compiled by
APE from the controlled-English equivalents below) and compares the active alerts frame by frame.
"""
import json
import random
import sys
from types import SimpleNamespace

sys.path.insert(0, sys.argv[1])
from yappu_fsm.vision.detector import COCO_CLASSES  # noqa: E402
from yappu_fsm.vision.events import EventEngine, Zone, parse_vision_rules  # noqa: E402

YAPPU_RULES = """
INTRUSION: If a person T is in the restricted-zone for at least 2 seconds then a zone-intrusion alert is raised for the person T.
PHONE: If the person T holds a cell-phone for at least 3 seconds then a phone-use alert is raised for the person T.
CROWD: If at least 2 persons are in the restricted-zone then a crowding alert is raised.
"""
# yappu event -> iogp rule id (the iogp sentences live in tools/yappu_parity_rules.ace)
MAP = {"zone-intrusion": "INTRUSION", "phone-use": "PHONE", "crowding": "CROWD"}

W, H, FPS, SECS, DROP = 640, 480, 10, 180, 0.25
PERSON, PHONE = COCO_CLASSES.index("person"), COCO_CLASSES.index("cell phone")


def main():
    rng = random.Random(459)
    eng = EventEngine(parse_vision_rules(YAPPU_RULES), [Zone("restricted", (0.0, 0.0, 0.4, 1.0))], grace=0.7, cooldown=3.0)
    people = {}
    for pid in range(1, 6):
        people[pid] = {"x": rng.uniform(0.05, 0.95), "vx": rng.uniform(-0.05, 0.05), "phone_until": -1.0}
    out = open(sys.argv[2], "w")
    n = SECS * FPS
    for i in range(n):
        t = round(i / FPS, 3)
        tracks = []
        for pid, p in people.items():
            # random walk with occasional dwell; phones picked up for random spells
            if rng.random() < 0.02:
                p["vx"] = rng.choice([0.0, 0.0, rng.uniform(-0.06, 0.06)])
            p["x"] = min(0.97, max(0.03, p["x"] + p["vx"] / FPS))
            if p["phone_until"] < t and rng.random() < 0.004:
                p["phone_until"] = t + rng.uniform(1.0, 8.0)
            if rng.random() < DROP:            # missed detection this frame
                continue
            cx = p["x"] * W
            box = (cx - 30, 150, cx + 30, 400)
            tracks.append(SimpleNamespace(tid=pid, cls=PERSON, box=box))
            if p["phone_until"] >= t and rng.random() >= DROP:
                tracks.append(SimpleNamespace(tid=100 + pid, cls=PHONE, box=(cx - 10, 250, cx + 10, 280)))
        eng.step(tracks, t, (W, H))
        objects = [{"id": "restricted", "zone": "restricted-area"}]
        facts = []
        for f in eng.last_facts:
            if f[0] == "person":
                objects.append({"id": f[1], "class": "person"})
            elif f[0] == "in_zone":
                facts.append(["in", f[1], f[2]])
            elif f[0] == "holds":
                oid = f"{f[2]}@{f[1]}"
                objects.append({"id": oid, "class": "phone" if f[2] == "cell-phone" else f[2]})
                facts.append(["hold", f[1], oid])
        active = sorted([MAP[e], ("restricted" if s == "scene" else s)] for s, e in eng.alerts.active())
        out.write(json.dumps({"t": t, "objects": objects, "facts": facts, "yappu_active": active}) + "\n")
    out.close()
    print(f"{n} frames written to {sys.argv[2]}")


if __name__ == "__main__":
    main()
