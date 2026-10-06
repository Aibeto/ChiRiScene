#!/usr/bin/env python3
"""基线归因第一层（阶段A 修订版，2026-10-07）：用现有 devimp 日志筛候选、校正口径。

只读，不写源码、不碰设备、不改原始日志。相对旧版（硬编码 CASES + 位置取值）的修订点：
  1. 必需 `--manifest` 显式输入；无参数或未知子命令报错，不默认回落 feas。
  2. `csv.DictReader` 按字段名取值；缺列/非法值/空值逐项计数，不静默丢弃。
  3. 时间轴按文件顺序展开跨午夜；回退不能判定为午夜时记 ambiguous 并切断连续段。
  4. screen_prop 作屏幕主层（原脚本的 q[5]=screen_on 不再沿用）；screen_on 仅作辅助，冲突单列。
  5. 差分只在「同文件、同段、双端身份/type/mode/package/screen_prop/clg_active 一致、CPU 非递减」内成立，
     分 strict-second(dt=1s) 与 short-interval(0<dt<10s) 两层；移除旧版无依据的 delta_cpu<=3000 过滤。
  6. 覆盖分母用有效间隔时长而非行数；未知时间不纳入均值；样本不足报 insufficient 而非 0。
  7. aff 帧保留日期/源文件/出现序号与全部重复秒内帧；@S 帧与 status 的时间关系作相位敏感性说明。
  8. diag 只承认显式开关证据为 confirmed；日志首尾只作文件覆盖证据，不填满中间秒。

区块索引: [cli] [manifest] [time] [status] [frames] [diff] [diag] [pair] [feas] [report]

用法（项目根目录执行）：
  python3 scripts/devimp/dvselfcost.py feas --manifest .trae/docs/2026-10-07-daemon-selfcost-inputs.json
  python3 scripts/devimp/dvselfcost.py diag --manifest ...
  python3 scripts/devimp/dvselfcost.py pair --manifest ...
"""
import argparse
import collections
import csv
import glob
import json
import os
import re
import sys

# [cli]
SUBCOMMANDS = ("feas", "diag", "pair")
UNKNOWN = "unknown"
STRICT_DT = 1.0
STRICT_TOL = 0.005
SHORT_MAX = 10.0
LONG_SEGMENT = 60.0
MIN_STRATUM = 5          # 分层自耗的「可比样本」下限，低于此报 insufficient
CASE_FIELDS = ("case_id", "status_path", "devimp_dir", "aff_path",
               "version_identity", "config_identity", "session_id")
REQUIRED_STATUS_FIELDS = ("timestamp", "type", "mode", "package", "screen_prop",
                          "clg_active", "daemon_utime_ms", "daemon_stime_ms")
# screen_prop = debug.tracing.screen_state 原始值；观测主导映射 prop∈{2,3,4} ↔ 亮屏，
# 仅用于「冲突单列」，不作静默过滤，也不代表已核对厂商语义。
SCREEN_PROP_ON = frozenset(("2", "3", "4"))
_PLACEHOLDERS = frozenset(("", "-", "--", "unknown", "n/a", "na", "null", "none"))
# diag 的显式开关证据（保守）：只有明确事件/配置字样才计 confirmed，日志首尾只算文件覆盖。
DIAG_OFF_MARKERS = (re.compile(r"dev_record\s*[:=]\s*(?:false|off|0)", re.I),
                    re.compile(r"开发记录.{0,8}(?:关闭|停用|停止)"),
                    re.compile(r"诊断采集.{0,8}(?:关闭|停止)"))
DIAG_ON_MARKERS = (re.compile(r"dev_record\s*[:=]\s*(?:true|on|1)", re.I),
                   re.compile(r"开发记录.{0,8}(?:开启|启用|开始)"),
                   re.compile(r"诊断采集.{0,8}(?:开启|开始)"))


class CaseError(Exception):
    """输入或结构不可用：明确失败，不回落默认值。"""


def clean(value):
    """空串/占位值（-/unknown 等）→ UNKNOWN，其余原样返回。"""
    text = (value or "").strip()
    return text if text and text.lower() not in _PLACEHOLDERS else UNKNOWN


def parse_float(text):
    try:
        value = float(text)
    except (TypeError, ValueError):
        return None
    return value if value == value and value not in (float("inf"), float("-inf")) else None


def parse_seconds(text):
    """'HH:MM:SS[.mmm]' → 当日秒（float）；非法返回 None。"""
    try:
        hours, minutes, seconds = text.split(":")
        hours, minutes, seconds = int(hours), int(minutes), float(seconds)
    except (ValueError, AttributeError):
        return None
    if not (0 <= hours < 24 and 0 <= minutes < 60 and 0 <= seconds < 60):
        return None
    return hours * 3600 + minutes * 60 + seconds


def repo_root():
    return os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def fmt_rate(rate):
    return "insufficient" if rate is None else "%.1f ms/s" % rate


def fmt_pct(numerator, denominator):
    """分母为 0（无可比样本）时报 insufficient，不报 0%。"""
    return "insufficient" if denominator <= 0 else "%.1f%%" % (100.0 * numerator / denominator)


# [manifest]
def resolve_path(root, value):
    return value if os.path.isabs(value) else os.path.normpath(os.path.join(root, value))


def load_manifest(path, root):
    """读清单：顶层 {"cases": [...]}，每项 A1 七字段；路径相对项目根解析；拒绝重复 case_id。"""
    try:
        with open(path, encoding="utf-8") as stream:
            data = json.load(stream)
    except OSError as error:
        raise CaseError("清单不可读: %s (%s)" % (path, error))
    except json.JSONDecodeError as error:
        raise CaseError("清单不是合法 JSON: %s" % error)
    cases = data.get("cases") if isinstance(data, dict) else None
    if not isinstance(cases, list) or not cases:
        raise CaseError('清单顶层必须为 {"cases": [...]} 且非空')
    seen, out = set(), []
    for index, item in enumerate(cases):
        if not isinstance(item, dict):
            raise CaseError("cases[%d] 不是对象" % index)
        missing = [field for field in CASE_FIELDS if field not in item]
        if missing:
            raise CaseError("cases[%d] 缺字段: %s" % (index, ",".join(missing)))
        case_id = str(item["case_id"]).strip()
        if not case_id:
            raise CaseError("cases[%d] case_id 为空" % index)
        if case_id in seen:
            raise CaseError("重复 case_id: %s" % case_id)
        seen.add(case_id)
        case = {"case_id": case_id}
        for key in ("status_path", "devimp_dir", "aff_path"):
            case[key] = resolve_path(root, str(item[key]))
        for key in ("version_identity", "config_identity", "session_id"):
            case[key] = clean(str(item[key]) if item[key] is not None else "")
        # 可选扩展：已知逻辑 CPU 数才建物理上界，缺失不擅自截断（A2）
        case["logical_cpus"] = item.get("logical_cpus")
        out.append(case)
    return out


def require_files(case):
    problems = []
    for key in ("status_path", "aff_path"):
        if not os.path.isfile(case[key]):
            problems.append("%s 文件不存在: %s" % (key, case[key]))
    if not os.path.isdir(case["devimp_dir"]):
        problems.append("devimp_dir 目录不存在: %s" % case["devimp_dir"])
    return problems


def observed_version(case):
    """从 status 同目录的 daemon.log 取模块版本行，仅作清单身份的旁证，不覆盖清单值。"""
    path = os.path.join(os.path.dirname(case["status_path"]), "daemon.log")
    if not os.path.isfile(path):
        return None
    try:
        with open(path, encoding="utf-8", errors="replace") as stream:
            for _ in range(400):
                line = stream.readline()
                if not line:
                    break
                if "模块版本:" in line:
                    return re.sub(r"[\u2068\u2069]", "", line.split("模块版本:", 1)[1]).strip()
    except OSError:
        return None
    return None


# [time]
class Timeline:
    """把同一文件内的行时间按出现顺序展开为绝对秒；回退不能判定为午夜即记 ambiguous 并切段。"""

    def __init__(self):
        self.offset = 0.0
        self.prev_raw = None
        self.segment = 0
        self.started = False
        self.midnight = 0
        self.ambiguous = 0
        self.unknown = 0

    def cut(self):
        """在未知/非法时间处切断连续段：不跨越未知点做差分。"""
        if self.started:
            self.segment += 1
        self.started = False
        self.prev_raw = None

    def step(self, raw):
        """→ (abs_seconds|None, seg_id, break_before)。不同文件/会话之间不共用本对象。"""
        if raw is None:
            self.unknown += 1
            self.cut()
            return None, self.segment, True
        brk = False
        if self.started:
            if raw < self.prev_raw:
                if self.prev_raw >= 18 * 3600 and raw <= 6 * 3600:
                    self.offset += 86400.0
                    self.midnight += 1
                else:
                    self.ambiguous += 1
                    brk = True
                    self.segment += 1
        elif self.segment > 0:
            brk = True
        self.started = True
        self.prev_raw = raw
        return raw + self.offset, self.segment, brk


# [status]
def identity(row):
    return (row["type"], row["mode"], row["package"], row["screen_prop"], row["clg_active"])


def load_status(case):
    """按字段名读 status.csv：缺列即失败；坏时间/坏 CPU 计数丢弃；空值计数保留。"""
    info = {"rows": [], "dropped": collections.Counter(), "empty": collections.Counter(),
            "crosstab": collections.Counter(), "conflicts": 0, "dup_second": 0,
            "timeline": Timeline(), "columns": []}
    with open(case["status_path"], encoding="utf-8", errors="replace", newline="") as stream:
        reader = csv.DictReader(stream)
        columns = reader.fieldnames or []
        info["columns"] = columns
        missing = [field for field in REQUIRED_STATUS_FIELDS if field not in columns]
        if missing:
            raise CaseError("status.csv 缺必需列: %s" % ",".join(missing))
        previous = None
        for row in reader:
            seconds = parse_seconds((row.get("timestamp") or "").strip())
            if seconds is None:
                info["dropped"]["bad_timestamp"] += 1
                info["timeline"].cut()
                continue
            utime = parse_float(row.get("daemon_utime_ms"))
            stime = parse_float(row.get("daemon_stime_ms"))
            if utime is None or stime is None:
                info["dropped"]["bad_cpu"] += 1
                info["timeline"].cut()
                continue
            for field in ("type", "mode", "package", "screen_prop", "clg_active", "screen_on"):
                if clean(row.get(field)) == UNKNOWN:
                    info["empty"][field] += 1
            abs_seconds, segment, brk = info["timeline"].step(seconds)
            record = {"abs": abs_seconds, "seg": segment, "break": brk, "raw": seconds,
                      "type": clean(row.get("type")), "mode": clean(row.get("mode")),
                      "package": clean(row.get("package")),
                      "screen_prop": clean(row.get("screen_prop")),
                      "screen_on": clean(row.get("screen_on")),
                      "clg_active": clean(row.get("clg_active")),
                      "cpu": utime + stime}
            info["rows"].append(record)
            info["crosstab"][(record["screen_on"], record["screen_prop"])] += 1
            if record["screen_prop"] != UNKNOWN and record["screen_on"] != UNKNOWN:
                if (record["screen_on"] == "1") != (record["screen_prop"] in SCREEN_PROP_ON):
                    info["conflicts"] += 1
            if previous is not None and previous["seg"] == segment \
                    and int(record["abs"]) == int(previous["abs"]):
                info["dup_second"] += 1
            previous = record
    return info


def segment_spans(rows):
    first, last = {}, {}
    for row in rows:
        if row["abs"] is None:
            continue
        first.setdefault(row["seg"], row["abs"])
        last[row["seg"]] = row["abs"]
    return {seg: last[seg] - first[seg] for seg in first}


# [frames]
FRAME_RE = re.compile(r"^@S\s+ts=(\d{4})-(\d{6})\s+ntop=(\d+)\s+nfg=(\d+)\s*(.*)$")


def load_frames(case):
    """读 aff @S 帧：保留日期、源文件、出现序号与全部重复秒内帧；不合并、不覆盖。"""
    frames, unmatched = [], 0
    timeline = Timeline()
    previous_date = None
    date_changes = 0
    with open(case["aff_path"], encoding="utf-8", errors="replace") as stream:
        for line in stream:
            if not line.startswith("@S"):
                continue
            match = FRAME_RE.match(line.rstrip("\n"))
            if not match:
                unmatched += 1
                continue
            date, hhmmss, ntop, nfg, tail = match.groups()
            raw = parse_seconds("%s:%s:%s" % (hhmmss[0:2], hhmmss[2:4], hhmmss[4:6]))
            abs_seconds, segment, brk = timeline.step(raw)
            if raw is not None:
                if previous_date is not None and date != previous_date:
                    date_changes += 1
                previous_date = date
            frames.append({"index": len(frames), "file": case["aff_path"], "date": date,
                           "raw": raw, "abs": abs_seconds, "seg": segment, "break": brk,
                           "ntop": int(ntop), "nfg": int(nfg), "full": "full=1" in tail})
    return {"frames": frames, "unmatched": unmatched, "timeline": timeline,
            "date_changes": date_changes}


# [diff]
class Layer:
    """一层差分的累计：均值用 sum(delta)/sum(dt)，分母是有效时长不是行数。"""

    def __init__(self):
        self.count = 0
        self.delta_sum = 0.0
        self.dt_sum = 0.0
        self.deltas = []

    def add(self, delta, dt):
        self.count += 1
        self.delta_sum += delta
        self.dt_sum += dt
        self.deltas.append(delta)

    @property
    def rate(self):
        return self.delta_sum / self.dt_sum if self.dt_sum > 0 else None


def iter_pairs(rows):
    """相邻行差分候选：跨段/回退/身份变化在此排除，不跨文件、不跨缺口。"""
    for a, b in zip(rows, rows[1:]):
        if b["seg"] != a["seg"]:
            yield a, b, None, "segment"
            continue
        dt = b["abs"] - a["abs"]
        if dt <= 0:
            yield a, b, dt, "nonmonotonic"
            continue
        if identity(a) != identity(b):
            yield a, b, dt, "boundary"
            continue
        yield a, b, dt, "ok"


def diff_layers(rows, logical_cpus=None):
    """→ (layers, stats, over_bound)。dt=1s 入 strict，0<dt<10s 入 short，其余入缺口。"""
    layers = {"strict": Layer(), "short": Layer()}
    stats = collections.Counter()
    over_bound = []
    for a, b, dt, reason in iter_pairs(rows):
        if reason != "ok":
            stats[reason] += 1
            if reason == "boundary" and dt is not None and 0 < dt < SHORT_MAX:
                stats["boundary_dt"] += dt
            continue
        if dt >= SHORT_MAX:
            stats["gap"] += 1
            stats["gap_dt"] += dt
            stats["max_gap"] = max(stats["max_gap"], dt)
            continue
        delta = b["cpu"] - a["cpu"]
        if delta < 0:
            stats["cpu_reset"] += 1
            continue
        if logical_cpus is not None:
            bound = float(logical_cpus) * dt * 1000.0
            if delta > bound:
                over_bound.append((a["abs"], delta, dt))
                continue
        (layers["strict"] if abs(dt - STRICT_DT) <= STRICT_TOL else layers["short"]).add(delta, dt)
    return layers, stats, over_bound


def strict_chains(rows):
    """严格连续层：{整数秒: 链号}；仅含 dt=1s、同身份、CPU 非递减、同段的相邻关系。"""
    chains, chain, prev = {}, 0, None
    for row in rows:
        if row["abs"] is None:
            continue
        key = int(row["abs"])
        if prev is not None and row["seg"] == prev["seg"] and row["cpu"] >= prev["cpu"] \
                and identity(row) == identity(prev) \
                and abs((row["abs"] - prev["abs"]) - STRICT_DT) <= STRICT_TOL:
            chains[key] = chain
        else:
            chain += 1
            chains[key] = chain
        prev = row
    return chains


def stratify(rows):
    """按同身份连续区间分层：→ [(key, 行列表)]，段内 dt≈1。"""
    groups = []
    current_key, current = None, []
    for a, b, dt, reason in iter_pairs(rows):
        if reason != "ok" or abs(dt - STRICT_DT) > STRICT_TOL:
            if current:
                groups.append((current_key, current))
                current_key, current = None, []
            continue
        if current_key is None:
            current_key = identity(a)
            current = [a]
        current.append(b)
    if current:
        groups.append((current_key, current))
    return groups


# [diag]
def coverage_seconds(directory, prefixes=("main_",)):
    """目录内 main_*.log 的覆盖秒（按文件展开跨午夜），只作「文件覆盖证据」。"""
    covered, timeline_report = set(), {"midnight": 0, "ambiguous": 0, "unknown": 0, "files": 0}
    for path in sorted(glob.glob(os.path.join(directory, "*.log"))):
        base = os.path.basename(path)
        if not any(base.startswith(prefix) for prefix in prefixes):
            continue
        timeline = Timeline()
        timeline_report["files"] += 1
        with open(path, encoding="utf-8", errors="replace") as stream:
            for line in stream:
                if not line or line[0] == "#":
                    continue
                match = re.match(r"(\d{2}:\d{2}:\d{2})", line)
                if not match:
                    continue
                abs_seconds, _seg, _brk = timeline.step(parse_seconds(match.group(1)))
                if abs_seconds is not None:
                    covered.add(int(abs_seconds))
        for key in ("midnight", "ambiguous", "unknown"):
            timeline_report[key] += getattr(timeline, key)
    return covered, timeline_report


def explicit_events(paths):
    """扫描显式开关证据：→ (on_seconds, off_seconds, matched_lines)。"""
    on, off, matched = set(), set(), []
    for path in paths:
        if not os.path.isfile(path):
            continue
        with open(path, encoding="utf-8", errors="replace") as stream:
            for line in stream:
                match = re.match(r"\[(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2})\]", line)
                if not match:
                    continue
                seconds = parse_seconds(match.group(2))
                if seconds is None:
                    continue
                text = re.sub(r"[\u2068\u2069]", "", line)
                if any(pattern.search(text) for pattern in DIAG_ON_MARKERS):
                    on.add(int(seconds))
                    matched.append(("on", match.group(2), text.strip()[:120]))
                elif any(pattern.search(text) for pattern in DIAG_OFF_MARKERS):
                    off.add(int(seconds))
                    matched.append(("off", match.group(2), text.strip()[:120]))
    return on, off, matched


# [pair]
def cmd_pair(cases, _args):
    for case in cases:
        info = load_status(case)
        rows = info["rows"]
        frames = load_frames(case)
        chains = strict_chains(rows)
        by_second = collections.defaultdict(list)
        for row in rows:
            if row["abs"] is not None:
                by_second[int(row["abs"])].append(row)
        per_second = collections.Counter(int(frame["abs"]) for frame in frames["frames"]
                                         if frame["abs"] is not None)
        report = new_report("pair", case)
        report.append("  aff 帧=%d 重复秒内帧=%d 未匹配@S行=%d"
                      % (len(frames["frames"]),
                         sum(count - 1 for count in per_second.values() if count > 1),
                         frames["unmatched"]))
        samples, rejected = [], collections.Counter()
        for second in sorted(per_second):
            if per_second[second] != 1:
                rejected["multi_frame"] += 1
                continue
            if second + 1 in per_second:
                rejected["adjacent_frame"] += 1
                continue
            window = [second, second + 1, second + 2]
            picked = [by_second.get(s) for s in window]
            if any(not rows_at or len(rows_at) != 1 for rows_at in picked):
                rejected["missing_endpoint"] += 1
                continue
            picked = [rows_at[0] for rows_at in picked]
            chain = chains.get(second)
            if chain is None or any(chains.get(s) != chain for s in window):
                rejected["not_strict_continuous"] += 1
                continue
            if any(row["screen_prop"] not in SCREEN_PROP_ON for row in picked):
                rejected["not_screen_on"] += 1
                continue
            frame_after = picked[1]["cpu"] - picked[0]["cpu"]
            frame_free = picked[2]["cpu"] - picked[1]["cpu"]
            previous = by_second.get(second - 1)
            frame_same = None
            if previous and len(previous) == 1 and chains.get(second - 1) == chain:
                frame_same = picked[0]["cpu"] - previous[0]["cpu"]
            samples.append({"second": second, "after": frame_after, "free": frame_free,
                            "same": frame_same})
        report.append("  严格连续亮屏跳帧配对 n=%d（拒绝: %s）"
                      % (len(samples),
                         ", ".join("%s=%d" % item for item in sorted(rejected.items())) or "无"))
        if len(samples) < MIN_STRATUM:
            report.append("  样本不足（< %d）: insufficient，不作帧成本估计" % MIN_STRATUM)
            emit(report)
            continue
        diff_after = [s["after"] - s["free"] for s in samples]
        report.append("  相位A（帧后一秒为帧成本）差 mean=%.1f median=%.1f ms；帧秒 %.1f / 非帧秒 %.1f"
                      % (sum(diff_after) / len(diff_after), median(diff_after),
                         mean([s["after"] for s in samples]), mean([s["free"] for s in samples])))
        same = [s for s in samples if s["same"] is not None]
        if len(same) >= MIN_STRATUM:
            diff_same = [s["same"] - s["free"] for s in same]
            report.append("  相位B（帧本秒为帧成本，n=%d）差 mean=%.1f median=%.1f ms"
                          % (len(same), mean(diff_same), median(diff_same)))
        report.append("  相位未唯一（秒级时间戳无法归属@S落在本秒还是下一秒）；"
                      "按符号自动断言原因一律不成立，本结果只作候选敏感性参考")
        emit(report)


# [feas]
def cmd_feas(cases, _args):
    identities = collections.Counter((case["version_identity"], case["config_identity"])
                                     for case in cases)
    if any(key[0] == UNKNOWN or key[1] == UNKNOWN for key in identities):
        print("警告: 存在未知身份（version/config=unknown），跨 unknown 身份不作比较")
    for case in cases:
        info = load_status(case)
        rows = info["rows"]
        frames = load_frames(case)
        layers, stats, over_bound = diff_layers(rows, case.get("logical_cpus"))
        spans = segment_spans(rows)
        span = sum(spans.values())
        valid_dt = layers["strict"].dt_sum + layers["short"].dt_sum
        report = new_report("feas", case)
        report.append("  status 行=%d 文件段数=%d 文件跨度=%.0fs 有效时长=%.0fs 覆盖=%s"
                      % (len(rows), len(spans), span, valid_dt, fmt_pct(valid_dt, span)))
        report.append("  缺口=%d 最大缺口=%.0fs 边界排除时长=%.0fs 重复秒=%d 时间非递进=%d "
                      "回退ambiguous=%d 未知时间=%d 计数重置=%d"
                      % (stats["gap"], stats["max_gap"], stats["boundary_dt"], info["dup_second"],
                         stats["nonmonotonic"], info["timeline"].ambiguous,
                         info["timeline"].unknown, stats["cpu_reset"]))
        report.append("  丢弃: 坏时间戳=%d 坏CPU=%d；空值: %s"
                      % (info["dropped"]["bad_timestamp"], info["dropped"]["bad_cpu"],
                         ", ".join("%s=%d" % item for item in sorted(info["empty"].items())) or "无"))
        for label, field in (("screen_prop", "screen_prop"), ("package", "package"),
                             ("mode", "mode"), ("clg_active", "clg_active")):
            report.append("  切换 %-11s %d 次" % (label, switch_count(rows, field)))
        report.append("  screen_on/screen_prop 交叉表: %s；冲突单列=%d（判据: screen_on==1 ↔ prop∈{2,3,4}）"
                      % (", ".join("so=%s/sp=%s:%d" % (key[0], key[1], count)
                                   for key, count in sorted(info["crosstab"].items())),
                         info["conflicts"]))
        lengths = [len(group) for _key, group in stratify(rows)]
        covered60 = sum(length for length in lengths if length >= LONG_SEGMENT)
        report.append("  同身份连续区间: 段数=%d 中位=%.0fs 最长=%.0fs；≥60s 段数=%d 覆盖=%.0fs(%s 有效时长)"
                      % (len(lengths), median(lengths), max(lengths) if lengths else 0,
                         sum(1 for length in lengths if length >= LONG_SEGMENT), covered60,
                         fmt_pct(covered60, valid_dt)))
        report.append("  两层差分: strict(dt=1s) n=%d %s；short(0<dt<10s) n=%d %s"
                      % (layers["strict"].count, fmt_rate(layers["strict"].rate),
                         layers["short"].count, fmt_rate(layers["short"].rate)))
        if over_bound:
            report.append("  物理上界外（logical_cpus=%s）n=%d 最大 delta=%.0fms @%.0fs"
                          % (case.get("logical_cpus"), len(over_bound),
                             max(item[1] for item in over_bound), over_bound[0][0]))
        else:
            report.append("  物理上界: 未提供 logical_cpus，按 A2 不擅自截断（只报异常最大值 %.0fms）"
                          % (max(layers["strict"].deltas + layers["short"].deltas)
                             if layers["strict"].deltas or layers["short"].deltas else 0))
        report.append("  分层自耗（strict 层，阈值 n>=%d 才给均值，否则 insufficient）:" % MIN_STRATUM)
        strata = collections.defaultdict(lambda: [0, 0.0, 0.0])
        for key, group in stratify(rows):
            for a, b in zip(group, group[1:]):
                bucket = strata[key]
                bucket[0] += 1
                bucket[1] += b["cpu"] - a["cpu"]
                bucket[2] += b["abs"] - a["abs"]
        for key in sorted(strata, key=lambda k: -strata[k][0])[:8]:
            count, delta_sum, dt_sum = strata[key]
            rate = delta_sum / dt_sum if dt_sum > 0 and count >= MIN_STRATUM else None
            report.append("    screen_prop=%s mode=%s clg=%s pkg=%s n=%d %s"
                          % (key[3], key[1], key[4], key[2], count, fmt_rate(rate)))
        report.append("  日志活动（落行次数≠执行次数，不作计数量）: aff 帧=%d 平均 nfg=%.1f "
                      "main 行=%d"
                      % (len(frames["frames"]),
                         mean([frame["nfg"] for frame in frames["frames"]]) if frames["frames"] else 0.0,
                         count_lines(case["devimp_dir"], "main_")))
        if frames["date_changes"] != frames["timeline"].midnight:
            report.append("  日期核对: @S 跨日=%d 与展开判定午夜=%d 不一致，相关段标 ambiguous"
                          % (frames["date_changes"], frames["timeline"].midnight))
        emit(report)


def cmd_diag(cases, _args):
    for case in cases:
        info = load_status(case)
        rows = info["rows"]
        covered, timeline_report = coverage_seconds(case["devimp_dir"])
        daemon = os.path.join(os.path.dirname(case["status_path"]), "daemon.log")
        on, off, matched = explicit_events([daemon])
        frames = load_frames(case)
        buckets = collections.defaultdict(lambda: [0, 0.0, 0.0])
        for a, b, dt, reason in iter_pairs(rows):
            if reason != "ok" or abs(dt - STRICT_DT) > STRICT_TOL or b["cpu"] < a["cpu"]:
                continue
            second = int(b["abs"])
            if second in off:
                state = "confirmed_off"
            elif second in on:
                state = "confirmed_on"
            elif second in covered:
                state = "coverage_only"
            else:
                state = "unknown"
            buckets[state][0] += 1
            buckets[state][1] += b["cpu"] - a["cpu"]
            buckets[state][2] += dt
        report = new_report("diag", case)
        report.append("  main 文件=%d 文件覆盖秒=%d（覆盖≠确认开启）；@S 未匹配=%d"
                      % (timeline_report["files"], len(covered), frames["unmatched"]))
        report.append("  显式开关证据: on=%d off=%d 命中行=%d；未命中即维持 unknown（不填满中间秒）"
                      % (len(on), len(off), len(matched)))
        for state in ("confirmed_on", "confirmed_off", "coverage_only", "unknown"):
            count, delta_sum, dt_sum = buckets[state]
            rate = delta_sum / dt_sum if dt_sum > 0 else None
            report.append("    %-13s n=%-6d %s" % (state, count, fmt_rate(rate)))
        report.append("  结论边界: 无 confirmed_off 样本时「诊断关闭期是否仍有自耗」不可判定，"
                      "需要新增对照采集；coverage_only 只作文件覆盖证据")
        emit(report)


# [report]
def version_code(text):
    match = re.search(r"versionCode\s*[:=]?\s*(\d+)", text or "")
    return match.group(1) if match else None


def new_report(command, case):
    lines = ["=" * 72, "%s | %s" % (command, case["case_id"])]
    lines.append("  version=%s config=%s session=%s" % (case["version_identity"],
                                                        case["config_identity"],
                                                        case["session_id"]))
    seen = observed_version(case)
    expected, observed = version_code(case["version_identity"]), version_code(seen or "")
    if expected and observed and expected != observed:
        lines.append("  清单 versionCode 与 daemon.log 不一致（旁证，不改清单）: %s" % seen)
    return lines


def emit(lines):
    print("\n".join(lines))


def mean(values):
    return sum(values) / len(values) if values else 0.0


def median(values):
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[len(ordered) // 2]


def switch_count(rows, field):
    """同段内相邻 1s 行的字段切换次数；不经过身份判定（否则被切换本身吞掉）。"""
    changes = 0
    for a, b in zip(rows, rows[1:]):
        if a["abs"] is None or b["abs"] is None or a["seg"] != b["seg"]:
            continue
        if abs((b["abs"] - a["abs"]) - STRICT_DT) > STRICT_TOL:
            continue
        if a[field] != b[field]:
            changes += 1
    return changes


def count_lines(directory, prefix):
    total = 0
    for path in glob.glob(os.path.join(directory, "*")):
        if os.path.basename(path).startswith(prefix):
            with open(path, encoding="utf-8", errors="replace") as stream:
                total += sum(1 for _ in stream)
    return total


def main(argv=None):
    parser = argparse.ArgumentParser(description="devimp 自耗归因第一层（阶段A 修订版）")
    parser.add_argument("command", choices=SUBCOMMANDS, help="feas / diag / pair，必需")
    parser.add_argument("--manifest", required=True, help="输入清单 JSON（顶层 cases）")
    parser.add_argument("--root", default=None, help="清单相对路径的解析根，缺省项目根")
    args = parser.parse_args(argv)
    root = args.root or repo_root()
    try:
        cases = load_manifest(args.manifest, root)
    except CaseError as error:
        print("错误: %s" % error, file=sys.stderr)
        return 2
    problems = []
    for case in cases:
        problems += ["[%s] %s" % (case["case_id"], problem) for problem in require_files(case)]
    if problems:
        for problem in problems:
            print("错误: %s" % problem, file=sys.stderr)
        return 2
    try:
        {"feas": cmd_feas, "diag": cmd_diag, "pair": cmd_pair}[args.command](cases, args)
    except CaseError as error:
        print("错误: %s" % error, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())