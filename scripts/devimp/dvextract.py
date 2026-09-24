#!/usr/bin/env python3
"""dvextract.py —— devimp 日志包解压（递归容忍，内容嗅探，不信任扩展名）。

用法:
    python scripts/devimp/dvextract.py <logd_*.tar.gz | 已解压目录> [--tag 名称] [--out-root devimpbin]

背景（2026-09-25 pack.sh 改动）：
- `module/scripts/pack.sh` 的 `archive` 分支把内层归档打成 **`.tar.lz4`**（`lz4 -f`）；
  设备上 lz4 不可用时**回落保留 `.tar`**。所以外层包里的内层成员可能是
  `<ts>.tar` / `<ts>.tar.lz4` / `devimp_<ts>.tar` / `devimp_<ts>.tar.lz4`——
  **一律按前 4 字节嗅探，不看名字**（见 已知坑：目录名/扩展名不可信）。
- 本机没装 `lz4` python 模块，走 `dvlz4.decompress_bytes` 的纯 python 解码器。

流程（与命令文档「1. 解压」一致）：
1. 外层 `logd_<MMDD-HHMMSS>.tar.gz` 解到 `devimpbin/<tag>/`；
2. 逐个内层成员按内容路由：LZ4 帧/遗留 magic → dvlz4；否则按 raw tar；
3. 任何「本身又是归档」的成员递归解到同级 `x_<stem>/`（去掉 .lz4/.tar 后的名字）；
4. **完整性闸门**：LZ4 解出后必须像 tar（`ustar` @257 或偏移 0 处名字合理），
   否则报错并同时给出压缩前/解出后两个字节数——这是没有 lz4 可交叉验证时唯一的自动护栏；
5. 校验每个 devimp 侧批次至少解出 main_*/aff_*；缺 logd 侧的批次高声告警
   （已知坑 12：老包因预算清理 bug 丢过 daemon.log/status.csv）；
6. 写 `inventory.txt`（每个文件 + 大小 + 所在嵌套层 + 检出压缩），stdout 只打摘要。可重复跑。

只写一个输出目录，可安全重跑（覆盖式重建）。
"""

import argparse
import io
import os
import re
import shutil
import sys
import tarfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dvlz4  # noqa: E402
import dvcommon as dc  # noqa: E402

MAGIC_LZ4_FRAME = 0x184D2204
MAGIC_LZ4_LEGACY = 0x184C2102
MAGIC_LZ4_SKIP = 0x184D2A50
MAGIC_GZIP = 0x8B1F  # 小端前两字节 1f 8b


def sniff(data):
    """按内容判定：'lz4' | 'gzip' | 'tar' | 'unknown'。前 4 字节优先。"""
    if len(data) >= 4:
        m = int.from_bytes(data[:4], "little")
        if m in (MAGIC_LZ4_FRAME, MAGIC_LZ4_LEGACY) or (m & 0xFFFFFFF0) == MAGIC_LZ4_SKIP:
            return "lz4"
    if len(data) >= 2 and int.from_bytes(data[:2], "little") == MAGIC_GZIP:
        return "gzip"
    if looks_like_tar(data):
        return "tar"
    return "unknown"


def looks_like_tar(data):
    """tar 的两个独立特征：偏移 257 的 'ustar'；或偏移 0 处一个合理的成员名。

    GNU/ustar 都有 ustar 魔数；POSIX 之外的旧 tar 退化为名字合理性检查
    （名字可打印非空、且 mode 字段八进制/空白）。
    """
    if len(data) < 512:
        return False
    if data[257:262] == b"ustar":
        return True
    name = data[:100].split(b"\x00", 1)[0]
    if not name or len(name) > 99:
        return False
    if not all(32 <= c < 127 for c in name):
        return False
    mode = data[100:108].split(b"\x00", 1)[0].strip()
    return mode == b"" or all(c in b"01234567 " for c in mode)


def decode_archive_bytes(raw, label):
    """把「外层成员」的原始字节解成 tar 字节：嗅探 → lz4/gzip/tar 直通。

    LZ4 路径带完整性闸门：解出的字节必须像 tar，否则抛错（附两个字节数）。
    """
    kind = sniff(raw)
    if kind == "lz4":
        try:
            out = dvlz4.decompress_bytes(raw)
        except Exception as e:  # 解码器自身报错也要带上下文
            raise RuntimeError(f"{label}: LZ4 解码失败（{len(raw)} bytes）: {e}") from e
        if not looks_like_tar(out):
            raise RuntimeError(
                f"{label}: LZ4 解码结果不是 tar（压缩前 {len(raw)} bytes → 解出 {len(out)} bytes，"
                f"头部 {out[:16]!r}）——解码器与真实编码器不一致，或文件已损坏"
            )
        return out, "tar.lz4"
    if kind == "gzip":
        import gzip
        out = gzip.decompress(raw)
        if not looks_like_tar(out):
            raise RuntimeError(
                f"{label}: gzip 解出结果不是 tar（{len(raw)} → {len(out)} bytes）")
        return out, "tar.gz"
    if kind == "tar":
        return raw, "tar"
    raise RuntimeError(f"{label}: 既不是 tar/lz4/gzip，也不是可识别的归档（前 16 字节 {raw[:16]!r}）")


def extract_tar_bytes(raw, dest):
    """把 tar 字节解到 dest，返回解出的常规文件名列表。"""
    os.makedirs(dest, exist_ok=True)
    names = []
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:") as tf:
        try:
            tf.extractall(dest, filter="data")
        except TypeError:  # 老 python 无 filter 参数
            tf.extractall(dest)
        for m in tf.getmembers():
            if m.isfile():
                names.append(os.path.basename(m.name))
    return names


def stem_of(name):
    """`devimp_0925-040608.tar.lz4` → `devimp_0925-040608`（剥 .lz4/.gz/.tar 各一层）。"""
    s = name
    for ext in (".lz4", ".gz", ".tar"):
        if s.endswith(ext):
            s = s[: -len(ext)]
    return s


def run(inp, tag, out_root, records):
    """递归解包；records 收集 (relpath, size, layer, compression)。

    返回 outdir。
    """
    is_dir = os.path.isdir(inp)
    if is_dir:
        outdir = os.path.abspath(inp)
        print(f"[i] 输入是已解压目录，跳过解压，只做清点：{outdir}")
        _inventory(outdir, records, [])
        return outdir, True

    if tag is None:
        m = re.search(r"(\d{4}-\d{6})", os.path.basename(inp))
        tag = m.group(1) if m else "pkg"
    outdir = os.path.abspath(os.path.join(out_root, tag))
    if os.path.exists(outdir):
        shutil.rmtree(outdir)  # 覆盖式重建：可安全重跑
    os.makedirs(outdir, exist_ok=True)

    with open(inp, "rb") as f:
        outer = f.read()
    if int.from_bytes(outer[:2], "little") == MAGIC_GZIP:
        import gzip
        outer_tar = gzip.decompress(outer)
    else:
        outer_tar = outer
    outer_kind = "tar.gz" if outer_tar is not outer else "tar"
    names = extract_tar_bytes(outer_tar, outdir)
    for n in names:
        records.append((n, os.path.getsize(os.path.join(outdir, n)), "0 outer", outer_kind))

    # 递归：把每个「本身是归档」的成员解到 x_<stem>/
    queue = [(os.path.join(outdir, n), 1, "outer") for n in names]
    seen = set()
    while queue:
        path, level, parent = queue.pop(0)
        rp = os.path.abspath(path)
        if rp in seen or not os.path.isfile(rp):
            continue
        seen.add(rp)
        with open(rp, "rb") as f:
            raw = f.read()
        kind = sniff(raw)
        is_archive = kind in ("lz4", "gzip", "tar")
        if not is_archive:
            continue
        try:
            inner, dec = decode_archive_bytes(raw, os.path.basename(rp))
        except RuntimeError as e:
            print(f"[!] {e}")
            records.append((os.path.relpath(rp, outdir), len(raw), f"L{level} 解包失败", kind))
            continue
        dest = os.path.join(os.path.dirname(rp), "x_" + stem_of(os.path.basename(rp)))
        try:
            got = extract_tar_bytes(inner, dest)
        except tarfile.TarError as e:
            print(f"[!] {os.path.basename(rp)}: 不是合法 tar（{len(raw)} → {len(inner)} bytes）: {e}")
            records.append((os.path.relpath(rp, outdir), len(raw), f"L{level} 解包失败", dec))
            continue
        layer = f"{level} {os.path.basename(dest)}"
        for n in got:
            p = os.path.join(dest, n)
            records.append((os.path.relpath(p, outdir), os.path.getsize(p), layer, sniff(open(p, "rb").read(8))))
            queue.append((p, level + 1, os.path.basename(dest)))
    return outdir, False


def _inventory(outdir, records, _unused):
    """已解压目录：清点 + 对每个归档文件补一条压缩类型（不递归解包）。"""
    for dirpath, _dirs, files in os.walk(outdir):
        for fn in files:
            p = os.path.join(dirpath, fn)
            rel = os.path.relpath(p, outdir)
            layer = os.path.relpath(dirpath, outdir)
            try:
                head = open(p, "rb").read(8)
            except OSError:
                head = b""
            records.append((rel, os.path.getsize(p), layer, sniff(head)))


def verify(outdir):
    """校验与告警：devimp 批次必须有 main_*/aff_*；缺 logd 侧的批次高声告警。"""
    warns = []
    devimp_dirs, logd_dirs = [], []
    for dirpath, _dirs, files in os.walk(outdir):
        base = os.path.basename(dirpath)
        if not base.startswith("x_"):
            continue
        names = set(files)
        if any(n.startswith(("main_", "aff_")) for n in names):
            devimp_dirs.append(base)
            if not any(n.startswith("main_") for n in names):
                warns.append(f"[!] {base}: devimp 侧没有 main_*.log（只剩 aff_）")
            if not any(n.startswith("aff_") for n in names):
                warns.append(f"[!] {base}: devimp 侧没有 aff_*.log（只剩 main_）")
        elif any(n in ("daemon.log", "status.csv") for n in names):
            logd_dirs.append(base)
    # 批次配对：x_<ts>（logd 侧）对 x_devimp_<ts>（devimp 侧）
    logd_ts = {b[len("x_"):] for b in logd_dirs}
    for b in devimp_dirs:
        ts = b[len("x_devimp_"):] if b.startswith("x_devimp_") else None
        if ts and ts not in logd_ts:
            warns.append(f"[!] devimp 批次 {b} 没有对应的 logd 侧（x_{ts}/：daemon.log/status.csv 缺失）"
                         f"——已知坑 12：老包预算清理 bug 产物，无 fps/FAS/charge/PowerAVG 判读")
    return warns, len(devimp_dirs), len(logd_dirs)


def main(argv=None):
    dc.setup_console()
    ap = argparse.ArgumentParser()
    ap.add_argument("input", help="logd_*.tar.gz 或已解压目录")
    ap.add_argument("--tag", default=None, help="输出子目录名（默认取包名 MMDD-HHMMSS）")
    ap.add_argument("--out-root", default="devimpbin")
    a = ap.parse_args(argv)

    records = []
    outdir, was_dir = run(a.input, a.tag, a.out_root, records)

    inv = ["# dvextract inventory",
           f"# input   : {os.path.abspath(a.input)}",
           f"# outdir  : {outdir}",
           f"# files   : {len(records)}",
           "#", f"{'size':>12s}  {'compression':<9s} {'layer':<38s} path"]
    for rel, size, layer, comp in sorted(records, key=lambda r: (r[2], r[0])):
        inv.append(f"{size:12d}  {comp:<9s} {layer:<38s} {rel}")
    warns, nd, nl = verify(outdir)
    inv.append("#")
    inv.append(f"# devimp 侧批次: {nd}   logd 侧批次: {nl}")
    inv.extend(warns)
    path = dc.write_report(outdir, "inventory.txt", inv)

    summary = [f"输出目录: {outdir}", f"文件数: {len(records)}  devimp 批次 {nd} / logd 批次 {nl}"]
    summary.extend(warns or ["无告警"])
    dc.announce("dvextract", path, summary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
