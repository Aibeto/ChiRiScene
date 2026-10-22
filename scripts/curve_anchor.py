#!/usr/bin/env python3
"""curve_anchor.py —— PF-Lite「参考能效曲线」离线生成器

区块索引: [args] [input] [curve] [check] [emit] [main]

定位（与 frontier_policy.py 并列，互不替代）:
  完整前沿生成器（frontier_policy.py）需要「逐档实测簇功耗 + 完整能效曲线」，
  它产出 46 点前沿 + 需求桶（离线算好、运行期只查表）。
  **本脚本只处理参考曲线**：输入是「参考功耗（必）+ 参考电压（可缺）」的锚点，
  不构造前沿、不落需求桶，只做三件事——
    1) 校验锚点（频点严格升序、功耗非减、电压非减、三列等长）；
    2) 用 P = Cx · f[GHz] · V[V]² 反解每簇 Cx（电压缺失时跳过，仅留功耗表）；
    3) 输出 `curve_anchor` YAML 段（受控块替换，块外逐字保留）。
  运行期由 src/chiri/frontier_lite.rs 按边际能效在线选点，**不读本脚本产物以外的任何输入**。

为什么 Lite 不落桶表: 参考曲线不是实测，把参考值固化成「需求桶→频点」会让后续拿到
真机 energy_model 时必须重挖。Lite 只用参考曲线做**相对排序**（哪档边际能效最高），
参考值的绝对偏差只影响排序边界、不影响机制正确性。

用法:
  python3 scripts/curve_anchor.py --soc sun --input <anchors.yaml>            # dry-run 打印
  python3 scripts/curve_anchor.py --soc sun --input <anchors.yaml> --write     # 受控块写入
  python3 scripts/curve_anchor.py --soc sun --input <anchors.yaml> --check     # 与现有块比对

锚点输入格式（YAML，纯标准库解析，见 [input]）:
  per_cluster:
    big:
      capacity: 837            # 可缺；缺则运行期用频点代理
      freq_khz: [384000, ...]  # 必填，严格升序
      power_w:  [0.42, ...]    # 必填，与 freq_khz 等长，非减
      voltage_mv: [520, ...]   # 可缺；有则须等长、非减
    prime: {...}
"""

import argparse
import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BLOCK_BEGIN = "# >>> frontier_policy (generated) >>>"
BLOCK_END = "# <<< frontier_policy <<<"
CLUSTERS = ("little", "big", "prime")

# [args]
# 目标机型 → soc.yaml 路径。新增机型 = 加一条。
SOC_PROFILES = {
    "sm8650": ROOT / "module/config/sm8650/soc.yaml",
    "sm8750": ROOT / "module/config/sm8750/soc.yaml",
}


def parse_args(argv):
    ap = argparse.ArgumentParser(description="PF-Lite 参考能效曲线生成器")
    ap.add_argument("--soc", required=True, help="机型键（见 SOC_PROFILES）")
    ap.add_argument("--input", required=True, help="锚点 YAML 路径")
    ap.add_argument("--write", action="store_true", help="写入 soc.yaml 受控块")
    ap.add_argument("--check", action="store_true", help="与现有块逐字节比对")
    ap.add_argument(
        "--disabled",
        action="store_true",
        help="写入数据但保持 enabled: false（数据先落地、暂不激活）",
    )
    ap.add_argument(
        "--force",
        action="store_true",
        help="锚点无 source 行时仍允许 --write（默认拒绝：防把占位/编造值写进受控块）",
    )
    return ap.parse_args(argv)


# [input]
def parse_source(text):
    """取锚点文件的 `source:` 行（数据出处）；缺失 → None。
    存在的意义：参考曲线一旦写进 soc.yaml 就会被当成「事实」参与选点排序，
    而占位/编造的功耗值同样能通过单调性校验，故必须要求写明出处。"""
    for raw in text.splitlines():
        m = re.match(r"^source:\s*(.+?)\s*$", raw)
        if m:
            return m.group(1)
    return None


def cluster_cores(soc_path: Path):
    """从 soc.yaml [topology] 读各簇核数（power_w 必须折算成簇级口径，见 emit_block）"""
    raw = soc_path.read_text(encoding="utf-8")
    m = re.search(r"^topology:\s*$", raw, re.M)
    if not m:
        return {}
    rest = raw[m.end():]
    stop = re.search(r"^[A-Za-z_]", rest, re.M)
    text = rest[: stop.start()] if stop else rest
    out = {}
    for c in CLUSTERS:
        mm = re.search(rf"^\s*{c}:\s*\{{\s*start:\s*(\d+),\s*end:\s*(\d+)", text, re.M)
        if mm:
            out[c] = int(mm.group(2)) - int(mm.group(1))
    return out


def parse_anchors(text):
    """极简 YAML 子集解析：只认 per_cluster.<簇>.{capacity,freq_khz,power_w,voltage_mv}。
    不依赖 pyyaml（本机 pyyaml 有损坏史，与 frontier_policy.py 同口径）。"""
    out = {}
    cur_cluster = None
    cur_key = None
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].rstrip()
        if not line.strip():
            continue
        stripped = line.strip()
        # 簇级键：`big:` / `little:` / `prime:`（缩进 2 或 4 都接受）
        m = re.match(r"^(\w+):\s*$", stripped)
        if m and m.group(1) in CLUSTERS:
            cur_cluster = m.group(1)
            out[cur_cluster] = {}
            cur_key = None
            continue
        if cur_cluster is None:
            continue
        # 标量键：capacity: 837
        m = re.match(r"^(\w+):\s*(\S+)\s*$", stripped)
        if m and m.group(1) == "capacity":
            out[cur_cluster]["capacity"] = int(m.group(2))
            cur_key = None
            continue
        # 序列键：freq_khz: [1, 2, 3] 或后续多行
        m = re.match(r"^(\w+):\s*\[(.*)$", stripped)
        if m:
            cur_key = m.group(1)
            body = m.group(2)
            out[cur_cluster][cur_key] = _nums(body)
            continue
        if cur_key:
            out[cur_cluster][cur_key].extend(_nums(stripped))
    return out


def _nums(s):
    return [float(x) for x in re.findall(r"-?\d+(?:\.\d+)?", s.replace("]", ""))]


# [curve]
def derive_cx(freq_khz, power_w, voltage_mv):
    """反解 Cx：P[W] = Cx · f[GHz] · V[V]² → Cx = P / (f·V²)，取各档中位数抗离群。
    电压缺失 / 长度不齐 → None（该簇只保留功耗表，不做模型化）。"""
    if not voltage_mv or len(voltage_mv) != len(freq_khz) or len(power_w) != len(freq_khz):
        return None
    cs = []
    for f, p, v in zip(freq_khz, power_w, voltage_mv):
        fg = f / 1e6
        vv = v / 1000.0
        if fg <= 0 or vv <= 0 or p <= 0:
            continue
        cs.append(p / (fg * vv * vv))
    if not cs:
        return None
    cs.sort()
    return round(cs[len(cs) // 2], 4)


# [check]
def validate(cluster, spec):
    """返回 (ok, 错误列表)。所有不齐一律拒绝，不猜、不截断。"""
    errs = []
    f = spec.get("freq_khz") or []
    p = spec.get("power_w") or []
    v = spec.get("voltage_mv") or []
    if not f:
        errs.append(f"{cluster}: freq_khz 缺失或为空")
    if len(p) != len(f):
        errs.append(f"{cluster}: power_w 长度 {len(p)} != freq_khz {len(f)}")
    if v and len(v) != len(f):
        errs.append(f"{cluster}: voltage_mv 长度 {len(v)} != freq_khz {len(f)}")
    if any(f[i] >= f[i + 1] for i in range(len(f) - 1)):
        errs.append(f"{cluster}: freq_khz 非严格升序")
    if any(p[i] > p[i + 1] for i in range(len(p) - 1)):
        errs.append(f"{cluster}: power_w 非单调非减（参考曲线必须随频率不降）")
    if v and any(v[i] > v[i + 1] for i in range(len(v) - 1)):
        errs.append(f"{cluster}: voltage_mv 非单调非减")
    return (not errs), errs


# [emit]
def emit_block(anchors, fingerprint, enabled=True, cores=None):
    """生成受控块正文（确定性：只由输入决定，不含时间戳）"""
    lines = [BLOCK_BEGIN, "frontier_policy:", f"  enabled: {'true' if enabled else 'false'}",
             f'  model_fingerprint: "{fingerprint}"',
             "  # 由 scripts/curve_anchor.py 生成（参考能效曲线，无实测表机型的唯一数据源）",
             "  per_cluster:"]
    for c in CLUSTERS:
        spec = anchors.get(c)
        if not spec:
            lines.append(f"    {c}: {{}}")
            continue
        f = [int(x) for x in spec.get("freq_khz", [])]
        # 【口径】锚点 power_w = **单核**动态功耗（图表 x 轴口径，已扣静态截距）；
        # 落表 per_cluster.power_w = **簇级** = 单核 × 该簇核数，与其余 37 个机型统一。
        # 依据：8550 little 落表首档 0.0690 W = 文档 §3「单核功耗」0.023 W × 3 核；
        # 且 §3 的「簇功耗 / 簇算力」两列都是簇级，生成器读的就是那两张表。
        # 不折算会让前沿 y（各簇功耗之和）把多核簇少算数倍，前沿形状与跨簇选点全失真。
        n = (cores or {}).get(c, 1) or 1
        p = [round(x * n, 4) for x in spec.get("power_w", [])]
        v = [int(x) for x in spec.get("voltage_mv", [])]
        cap = spec.get("capacity")
        cx = derive_cx(f, p, v)
        parts = []
        if cap is not None:
            parts.append(f"capacity: {cap}")
        parts.append("freq_khz: [" + ", ".join(str(x) for x in f) + "]")
        parts.append("power_w: [" + ", ".join(str(x) for x in p) + "]")
        # 单核口径：运行期相对能效模型直接消费；与 power_w 除以核数的结果一致
        single = [round(x, 4) for x in spec.get("power_w", [])]
        parts.append("single_core_power_w: [" + ", ".join(str(x) for x in single) + "]")
        if v:
            parts.append("voltage_mv: [" + ", ".join(str(x) for x in v) + "]")
        if cx is not None:
            parts.append(f"# Cx≈{cx}（P=Cx·f[GHz]·V[V]² 反解中位数）")
        body = ", ".join(x for x in parts if not x.startswith("#"))
        lines.append(f"    {c}: {{ {body} }}")
        if cx is not None:
            lines.append(f"    # {c} 参考模型: Cx≈{cx}")
    lines.append(BLOCK_END)
    return "\n".join(lines) + "\n"


def replace_block(text, block):
    """块内替换、块外逐字保留。

    三种情形（顺序有语义）：
      1. 已有受控块标记 → 块内替换（常规路径，幂等）；
      2. **无标记但已存在同名顶层键** → 替换该键所在段。
         这一步不能省：本段的初版是**手写**的（没有 generated 标记），若按「无标记就追加」
         处理，文件里会出现**两个同名 `curve_anchor:` 键**，serde_yaml 报
         `duplicate field` → `soc_config()` 整份解析失败 → 连 topology / freq_khz 兜底
         一起丢掉（8e 会静默退化成硬编码布局 0..3/3..7/7..8，对本机全错）。
         已用 serde_yaml 0.9 实测确认该行为。
      3. 都没有 → 追加到文件末尾。
    """
    if BLOCK_BEGIN in text and BLOCK_END in text:
        head, rest = text.split(BLOCK_BEGIN, 1)
        _, tail = rest.split(BLOCK_END, 1)
        return head + block + tail.lstrip("\n")

    lines = text.splitlines(keepends=True)
    start = next(
        (i for i, ln in enumerate(lines) if re.match(r"^frontier_policy\s*:", ln)),
        None,
    )
    if start is not None:
        # 段尾 = 下一行「非空且顶格」；空行与缩进行都属于本段
        end = start + 1
        while end < len(lines):
            nxt = lines[end]
            if nxt.strip() == "" or nxt[0] in " \t":
                end += 1
                continue
            break
        # 尾部空行还给下一段，避免吃掉分节空行
        while end > start + 1 and lines[end - 1].strip() == "":
            end -= 1
        return "".join(lines[:start]) + block + "\n" + "".join(lines[end:])

    sep = "" if text.endswith("\n\n") else ("\n" if text.endswith("\n") else "\n\n")
    return text + sep + block


# [main]
def main(argv=None):
    args = parse_args(argv or sys.argv[1:])
    target = SOC_PROFILES.get(args.soc)
    if target is None:
        print(f"未知机型键 {args.soc}；已注册: {', '.join(SOC_PROFILES)}", file=sys.stderr)
        return 2
    if not target.exists():
        print(f"目标 soc.yaml 不存在: {target}", file=sys.stderr)
        return 2
    src = Path(args.input)
    if not src.exists():
        print(f"锚点文件不存在: {src}", file=sys.stderr)
        return 2

    text = src.read_text(encoding="utf-8")
    anchors = parse_anchors(text)
    all_errs = []
    for c in CLUSTERS:
        if c in anchors:
            ok, errs = validate(c, anchors[c])
            if not ok:
                all_errs.extend(errs)
    if all_errs:
        print("锚点校验失败：", file=sys.stderr)
        for e in all_errs:
            print("  - " + e, file=sys.stderr)
        return 1
    if not any(anchors.get(c, {}).get("freq_khz") for c in CLUSTERS):
        print("锚点为空：至少给一个簇的 freq_khz/power_w", file=sys.stderr)
        return 1

    fp = hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]
    block = emit_block(
        anchors, fp, enabled=not args.disabled, cores=cluster_cores(target)
    )
    cur = target.read_text(encoding="utf-8")

    if args.check:
        if BLOCK_BEGIN in cur and BLOCK_END in cur:
            have = cur.split(BLOCK_BEGIN, 1)[1].split(BLOCK_END, 1)[0]
            want = block.split(BLOCK_BEGIN, 1)[1].split(BLOCK_END, 1)[0]
            if have.strip() == want.strip():
                print("check OK：现有块与生成结果一致")
                return 0
        print("check FAILED：现有块与生成结果不一致（重跑 --write）", file=sys.stderr)
        return 1

    if args.write:
        src_note = parse_source(text)
        if src_note is None and not args.force:
            print(
                "拒绝写入：锚点文件缺少 `source:` 行（写明数据出处，如真机 debugfs 导出/补丁 URL）。\n"
                "  参考曲线会被当作选点排序依据，占位值同样能通过单调性校验，故强制要求出处。\n"
                "  确认要写入占位值：加 --force。",
                file=sys.stderr,
            )
            return 1
        out = replace_block(cur, block)
        # 自检：顶层同名键必须恰好一次（重复键会让 serde_yaml 整份解析失败）
        n = len(re.findall(r"^frontier_policy\s*:", out, flags=re.M))
        if n != 1:
            print(f"拒绝写入：生成结果含 {n} 个顶层 frontier_policy 键（必须恰好 1 个）", file=sys.stderr)
            return 1
        target.write_text(out, encoding="utf-8")
        print(f"已写入 {target}（fingerprint {fp}）")
    else:
        print(block)
        print(f"# dry-run（未写入）；fingerprint {fp}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
