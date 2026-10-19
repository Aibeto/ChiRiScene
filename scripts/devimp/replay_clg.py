"""replay_clg.py: [parse] [source] [cli] -- source-backed sampled replay."""
import argparse
import collections
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]

# [parse] Logged changed samples only; screen is never inferred from snap rows.
def parse_rows(stream):
    counts = collections.Counter()
    rows, seen = [], set()
    segments = collections.defaultdict(int)
    last_time = {}
    boundary = False
    reader = csv.DictReader(line for line in stream if not line.startswith("#"))
    for row in reader:
        if row.get("type") != "tick":
            if row.get("mode") not in (None, "-", "default"):
                boundary = True
            continue
        counts["all_ticks"] += 1
        if row.get("mode") == "fas":
            counts["fas_ticks"] += 1
        if row.get("mode") != "default":
            counts["nondefault_ticks"] += 1
            boundary = True
            continue
        try:
            util = float(row["max_util"])
            if not math.isfinite(util) or not 0 <= util <= 1 or row["cluster"] not in ("little", "big", "prime"):
                raise ValueError("invalid tick")
        except (ValueError, KeyError, TypeError):
            counts["invalid"] += 1
            continue
        signature = tuple(sorted(row.items()))
        if signature in seen:
            counts["duplicates"] += 1
            continue
        seen.add(signature)
        group = row["cluster"]
        try:
            hour, minute, second = row["ts"].split(":")
            timestamp = int(hour) * 3600 + int(minute) * 60 + float(second)
        except (KeyError, ValueError):
            counts["invalid"] += 1
            boundary = True
            continue
        if boundary:
            for known in last_time:
                segments[known] += 1
            last_time.clear()
            boundary = False
        if group in last_time and not 0 <= timestamp - last_time[group] <= 1.0:
            segments[group] += 1
            counts["gap_resets"] += 1
        last_time[group] = timestamp
        row["_segment"] = segments[group]
        counts["unknown_screen"] += row.get("screen_on") == "-"
        rows.append(row)
    return rows, counts

# [source] Extract complete brace-balanced functions unchanged; no decision copy.
def function(text, signature):
    start = text.index(signature)
    brace = text.index("{", start)
    depth = 1
    end = brace + 1
    # Current selected functions have no brace-containing string/comment literals.
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]

def build_source():
    production = (ROOT / "src/chiri/cpu_load_governor.rs").read_text()
    config_source = (ROOT / "src/chiri/config.rs").read_text()
    feature = (ROOT / "module/config/sm8550/feature.yaml").read_text()
    soc = (ROOT / "module/config/sm8550/soc.yaml").read_text()
    decision = function(production, "fn on_load_update(&mut self, core_utils: &[f32])")
    helpers = "\n".join(function(production, "fn " + name) for name in (
        "find_floor_freq(&self, target_ratio: f32)", "deadzone_band(&self, deadzone_ratio: f32)",
        "max_util(&self, core_utils: &[f32])", "ratio_of_freq(&self, freq: u32)"))
    fields = sorted(set(re.findall(r"self.cfg.(\w+)", decision))
                    | set(re.findall(r"self\.cfg\.(\w+)", helpers.split("fn max_util", 1)[-1]))
                    | {"perf_init"})
    block = feature.split("# [default]")[1].split("# [boost]")[0]
    scalars = dict(re.findall(r"^    (\w+): ([0-9.]+|true|false)$", block, re.M))
    declarations, values = [], []
    for field in fields:
        match = re.search(r'#\[serde\(default(?: = "([^"]+)")?\)\]\s+pub ' + field + r': (\w+)', config_source)
        default_fn, kind = match.groups()
        value = scalars.get(field)
        if value is None:
            value = "true" if default_fn == "crate::utils::default_true" else re.search(r"\{\s*([0-9.]+)\s*\}", function(config_source, "fn " + default_fn + "()")).group(1)
        declarations.append(f"{field}: {kind},")
        values.append(f"{field}: {value},")
    cfg = "struct CpuLoadGovernorConfig {" + "".join(declarations) + "}\n"
    little = re.search(r"^      little:\n        up_threshold: ([0-9.]+)", block, re.M).group(1)
    cfg += 'fn config(group: &str) -> CpuLoadGovernorConfig { let mut c = CpuLoadGovernorConfig {' + "".join(values) + '}; if group=="little" {c.up_threshold=' + little + ';} c }\n'
    freq_block = soc.split("freq_khz:")[1].split("# [idle]")[0]
    cfg += 'fn frequencies(group: &str) -> Vec<u32> { match group {'
    for group in ("little", "big", "prime"):
        numbers = re.search(r"  " + group + r":\s*\[([^]]+)\]", freq_block).group(1)
        cfg += '"' + group + '" => vec![' + numbers + '],'
    cfg += '_ => panic!("unknown cluster"),} }'
    template = (ROOT / "scripts/tests/clg_replay.rs").read_text()
    return template.replace("/* CONFIG */", cfg).replace("/* HELPERS */", helpers).replace("/* DECISION */", decision)

# [cli] Sources and all generated artifacts stay within the project.
def percentile(values, p):
    values = sorted(values)
    return values[math.ceil(p * len(values)) - 1] if values else None

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "devimpbin/p23_replay.json")
    args = parser.parse_args()
    root, output = args.root.resolve(), args.output.resolve()
    if not root.is_relative_to(ROOT) or not output.is_relative_to(ROOT):
        parser.error("input/output must stay within project")
    scratch = ROOT / "devimpbin/p23_scratch.rs"
    binary = ROOT / "devimpbin/p23_replay"
    tmp = ROOT / "devimpbin/p23_tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, TMPDIR=str(tmp))
    source = build_source()
    scratch.write_text(source)
    try:
        subprocess.run(["rustc", "--edition=2024", "-O", str(scratch), "-o", str(binary)], check=True, env=env)
        check = subprocess.check_output([str(binary), "--selfcheck"], text=True)
        batches = []
        for batch in sorted(root.glob("x_devimp_*")):
            payload, counts = [], collections.Counter()
            original = collections.Counter()
            original_freq = []
            files = sorted(batch.glob("main_*.log"))
            for file_index, path in enumerate(files):
                with path.open() as stream:
                    rows, parsed = parse_rows(stream)
                counts.update(parsed)
                # Per-cluster file sequences: no cross-package-file or batch interpolation.
                groups = collections.defaultdict(list)
                for row in rows:
                    groups[(row["cluster"], row["_segment"])].append(row)
                    original[row["decision"]] += 1
                    original_freq.append(int(row["cur_freq_khz"]))
                for (group, segment), samples in groups.items():
                    for row in samples:
                        payload.append(f"{file_index}-{group}-{segment},{group},{row['max_util']}")
            proc = subprocess.run([str(binary)], input="\n".join(payload), text=True, capture_output=True, check=True)
            decisions, freq, steady = collections.Counter(), [], 0
            for line in proc.stdout.splitlines():
                _, decision, target, khz, decay = line.split(',')
                decisions[decision] += 1
                freq.append(int(khz))
                steady += float(decay) > 0
            batches.append(dict(batch=batch.name, files=len(files), samples=len(payload), counts=dict(counts),
                recorded_decisions=dict(original), replay_decisions=dict(decisions),
                recorded_freq_p95=percentile(original_freq, .95), recorded_freq_p99=percentile(original_freq,.99),
                ratio_sink_freq_p95=percentile(freq,.95), ratio_sink_freq_p99=percentile(freq,.99), steady_decay_samples=steady))
        report = dict(schema="clg-sampled-replay/1", source_sha256=hashlib.sha256(source.encode()).hexdigest(),
            boundaries="default changed tick samples only; screen_on unknown; FAS has no ticks; no reconstructed missing ticks; file/cluster/mode-boundary and gaps >1s or clock reversal reset; input util fixed, no frequency feedback; ideal ratio sink excludes PF, touch, thermal, dwell, sysfs and per-core counts; not real-device response/power evidence; recorded distribution is observational, not old-config replay or A/B", selfcheck=check.splitlines(), batches=batches)
        output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps(report, ensure_ascii=False, indent=2))
    finally:
        scratch.unlink(missing_ok=True)

if __name__ == "__main__":
    main()
