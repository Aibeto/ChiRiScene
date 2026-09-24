#!/usr/bin/env python3
r"""dvcommon.py —— devimp 日志分析脚本的共享工具（纯标准库）。

区块索引: [schema] [head] [rows] [files] [stats] [output]

定位：`.cursor/commands/devimp-log-analysis.md`（判读口径全集）+ `scripts/devimp-analyze.py`
（列表现行定义）。本模块只做「读文件、切行、算统计、写报告」的公共部分，
**不重新解释任何判定口径**——口径一律以命令文档「三、判定要点」为准。

约定（刻意为之，勿改）：
- 每个脚本自己把完整结果写进 UTF-8 文件，stdout 只打几行摘要 + 输出路径。
  原因：PowerShell 重定向会把 python stdout 按控制台代码页重编码，中文必乱码
  （2026-09-25 实测：重定向后产出 mojibake）。因此**禁止**依赖 shell 重定向取结果。
- schema 判定与列索引与 `scripts/devimp-analyze.py` 一致（C48/C44/C40 直接抄自该文件）。
- 行过滤一律用 `^\d{2}:\d{2}:\d{2}\.\d{3},`（ts 无日期），不要按日期前缀判断。
"""

import collections
import glob
import os
import re
import sys

# [schema]
# ── 列定义**抄自 scripts/devimp-analyze.py（该文件是列口径唯一真源，勿在此处另立一套）** ──
C48 = ("ts,type,mode,screen_on,pid,package,tid,comm,cluster,core,from_core,to_core,util_pct,max_util,"
       "over_cores,under_cores,cur_perf,tgt_perf,cur_freq_khz,max_freq_khz,decision,deb_up,deb_down,"
       "reason,pinned,thermal_cap_pct,touch,psi_cpu,psi_io,psi_mem,gpu_busy,batt_v,batt_i,batt_p,"
       "wakeups,migrations,freq_trans,batt_temp,cpu_temp,clg_active,cpu_cur_khz,cpu_max_khz,cpu_min_khz,"
       "cpu_governor,gpu_cur_khz,gpu_max_khz,gpu_min_khz,gpu_governor").split(",")
C44 = [c for c in C48 if c not in ("from_core", "to_core", "util_pct", "pinned")]
# 40 列旧版（Canary92 之前，如 8550e/Canary88）：= 48 列去掉尾部 8 列（cpu_cur_khz…gpu_governor）
C40 = C48[:40]


def detect_schema(header):
    """首行表头 → (cols, tag)。判定口径同 devimp-analyze.py：
    无 cpu_cur_khz = 40 列旧；有 cpu_cur_khz 且含 from_core = 48 列；否则 44 列新。"""
    if "cpu_cur_khz" not in header:
        return C40, "40列旧(Canary92前)"
    if "from_core" in header:
        return C48, "48列(Canary92期)"
    return C44, "44列新(拆分版)"


def index_map(cols):
    return {n: i for i, n in enumerate(cols)}


# [head]
_HEAD_MODULE = re.compile(r"#\s*module=(.+?)\s*$")
_HEAD_SOC = re.compile(r"#\s*soc=(\S+)\s+board=(\S+)\s+model=(.+?)\s*$")
_HEAD_ANDROID = re.compile(r"#\s*android=(\S+)\s+\(sdk (\d+)\)\s+kernel=(.+?)\s*$")


def parse_head(path, nlines=14):
    """读 devimp 文件头（与 status/daemon 无关）。

    返回 dict：module/soc/board/model/android/sdk/kernel/ts_column/header/schema_tag/cols。
    文件头第 1 行是 CSV 表头或 aff_ 说明，元信息是紧随其后的 `#` 行。旧包可能没有元信息。
    """
    out = {"module": None, "soc": None, "board": None, "model": None,
           "android": None, "sdk": None, "kernel": None, "ts_column": None,
           "header": "", "schema_tag": None, "cols": None}
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            for _ in range(nlines):
                ln = fh.readline()
                if not ln:
                    break
                s = ln.strip()
                if not out["header"] and s and not s.startswith("#"):
                    out["header"] = s
                m = _HEAD_MODULE.match(s)
                if m:
                    out["module"] = m.group(1)
                    continue
                m = _HEAD_SOC.match(s)
                if m:
                    out["soc"], out["board"], out["model"] = m.group(1), m.group(2), m.group(3)
                    continue
                m = _HEAD_ANDROID.match(s)
                if m:
                    out["android"], out["sdk"], out["kernel"] = m.group(1), m.group(2), m.group(3)
                    continue
                if s.startswith("# ts-column="):
                    out["ts_column"] = s[len("# ts-column="):]
    except OSError:
        pass
    if out["header"]:
        out["cols"], out["schema_tag"] = detect_schema(out["header"])
    return out


def fingerprint(heads):
    """把多个 parse_head 结果合成 {字段: set(值)}，用于混版本/混机型报警。"""
    fp = collections.defaultdict(set)
    for h in heads:
        for k in ("module", "soc", "board", "model", "android", "kernel", "schema_tag"):
            if h.get(k):
                fp[k].add(h[k])
    return dict(fp)


# [rows]
TS_RE = re.compile(r"^\d{2}:\d{2}:\d{2}\.\d{3},")


def iter_rows(path, min_cols=40):
    """逐行 yield (fields, row_type)；跳过表头/注释/短行。row_type 取第 1 列。

    ts 无日期（`# ts-column=local format_now`），**不要**用日期前缀过滤。
    """
    with open(path, encoding="utf-8", errors="replace") as fh:
        for ln in fh:
            if not ln or ln[0] == "#" or not TS_RE.match(ln):
                continue
            p = ln.rstrip("\n").split(",")
            if len(p) < min_cols:
                continue
            yield p, p[1]


# [files]
def file_stamp(path):
    """从 `main_<pkg>_MMDD-HHMMSS[-N].log` / `aff_MMDD-HHMMSS[-N].log` 取 MMDD-HHMMSS。"""
    m = re.search(r"_(\d{4}-\d{6})(?:-\d+)?\.log$", os.path.basename(path))
    return m.group(1) if m else None


def list_files(root, prefixes, since="0000-000000"):
    """递归收集 root 下 basename 以 prefixes 之一开头的 .log，按时间戳过滤 >= since。

    与聚合脚本同口径：`--since MMDD-HHMMSS` 按**文件名时间戳**筛，避免混入旧簇。
    """
    found = []
    for fn in glob.glob(os.path.join(root, "**", "*.log"), recursive=True):
        base = os.path.basename(fn)
        if not any(base.startswith(p) for p in prefixes):
            continue
        st = file_stamp(fn)
        if st and st >= since:
            found.append(fn)
    found.sort()
    return found


def batches(files):
    """按父目录分组 = 一次导出的「簇」（x_devimp_<ts>/ 或 x_<ts>/）。"""
    g = collections.defaultdict(list)
    for fn in files:
        g[os.path.dirname(fn)].append(fn)
    return dict(g)


# [stats]
def num(v):
    try:
        return float(v)
    except Exception:
        return None


def avg(v):
    v = [x for x in v if x is not None]
    return sum(v) / len(v) if v else 0.0


def pct(v, q):
    v = sorted(x for x in v if x is not None)
    return v[min(int(len(v) * q), len(v) - 1)] if v else 0.0


def parse_policies(s):
    """`policy0:787200;policy3:1785600;policy7:595200` → {'policy0': 787200, ...}。

    44/48 列的 cpu_cur_khz / cpu_max_khz / cpu_min_khz 与 thermal 相关列都是这个串格式。
    """
    out = {}
    if not s or s == "-":
        return out
    for part in s.split(";"):
        if ":" not in part:
            continue
        k, _, v = part.partition(":")
        n = num(v)
        if n is not None:
            out[k.strip()] = n
    return out


# [output]
def strip_isolates(s):
    """去掉 U+2068/U+2069 隔离符（daemon.log 里 `P⁨0⁩`、`⁨playback⁩`）。
    不先剥符，`P(\\d+)`、`mode=⁨…⁩` 之类正则全部匹配不到（已知坑 10）。"""
    return re.sub(r"[\u2068\u2069]", "", s)


def write_report(workdir, name, lines):
    """把完整结果写进 workdir/name（UTF-8, LF），返回绝对路径。"""
    path = os.path.join(workdir, name)
    os.makedirs(workdir, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        if lines:
            f.write("\n".join(str(x) for x in lines))
            f.write("\n")
    return path


def announce(title, path, summary):
    """只打几行摘要 + 输出路径（**不要**把全文打到 stdout：重定向会毁掉中文）。"""
    print(f"== {title} -> {path}")
    for s in summary:
        print(f"   {s}")


def setup_console():
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass


def bootstrap_path():
    """把脚本自身目录插进 sys.path，使 `python scripts/devimp/xxx.py` 与被 import 都可用。"""
    here = os.path.dirname(os.path.abspath(__file__))
    if here not in sys.path:
        sys.path.insert(0, here)
    return here


def repo_root():
    return os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
