#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""devreflow.py —— 注释折行重排的「粘结」定位器（只读，不改任何文件）。

区块索引: [const] [read] [map] [junc] [drop] [cli]

背景：把多行注释合并成一行时若顺手删掉行末标点或换行，句子会粘成「状态变化变化转发」
（原文「状态变化。变化转发：」），`ro.soc.model` 这类标识符也会被吃掉点号。
本工具逐文件比对基线提交与工作区，只**列出**候选点与上下文，改由人工逐处确认。

口径:
- 只处理注释块（`//` `#` `/* */` 内文），行首标记不进比对。
- 两类候选：① 折行交界处两个片段在新文本里紧贴（同行相邻）→ 粘结；
  ② 单个旧注释行内有标点被删（局部对齐，相似度门槛内）→ 标点丢失。
- 只报告，不写出文件。

用法:
    python scripts/devreflow.py list --base e1a27ab [--out FILE] [--expr src]
"""

import argparse
import difflib
import io
import subprocess
import sys
from pathlib import Path

# [const]
MARKERS = ("///", "//!", "//", "/*", "*/", "<!--", "-->", "*", "#", "--", ";", "%")
SPACE = set(" \t\r\n")
PUNCT = set(
    "。，、；：？！…—·「」『』（）《》〈〉“”‘’【】"
    ",.;:!?()[]{}<>\"'`~@$%^&*+-=/\\|_"
)
SEP_PUNCT = set("。，、；：？！…")  # 句读类：交界处丢了它才算粘结
MIN_ANCHOR = 6  # 片段锚点至少这么长，太短的不可靠
MIN_PAYLOAD = 0  # 旧行正文最短长度（骨架字符）
DROP_RATIO = 0.93  # 行内标点丢失的局部相似度门槛
MAX_BLOCK = 400


def run_git(args):
    out = subprocess.run(
        ["git"] + args, capture_output=True, cwd=Path(__file__).resolve().parent.parent
    )
    if out.returncode != 0:
        raise SystemExit("git 失败: git " + " ".join(args) + "\n" + out.stderr.decode("utf-8", "replace"))
    return out.stdout


# [read]
def read_pair(rel, base):
    old_bytes = run_git(["show", base + ":" + rel])
    path = Path(__file__).resolve().parent.parent / rel
    if not path.exists():
        return None, None
    old = old_bytes.decode("utf-8", "replace").splitlines(keepends=True)
    new = path.read_text(encoding="utf-8", newline="").splitlines(keepends=True)
    return old, new


def strip_marker(line):
    s = line.strip()
    for m in MARKERS:
        if s.startswith(m):
            rest = s[len(m) :]
            if m == "#" and rest.startswith("["):
                return None
            return rest
    return None


def comment_flags(lines):
    flags = []
    in_block = False
    for ln in lines:
        s = ln.strip()
        if in_block:
            flags.append(True)
            if "*/" in s:
                in_block = False
            continue
        if s.startswith("/*"):
            flags.append(True)
            if "*/" not in s[2:]:
                in_block = True
            continue
        flags.append(strip_marker(ln) is not None)
    return flags


# [map] 注释块 → 行正文清单 / 正文拼接与「非空字符→(行下标, 行内偏移)」映射
def payloads_of(lines, i1, i2):
    out = []
    for li in range(i1, i2):
        raw = lines[li].rstrip("\r\n")
        body = strip_marker(raw)
        if body is None:
            body = raw.strip()
        if body:
            col = raw.find(body, len(raw) - len(raw.lstrip()))
            out.append((li, body, col if col >= 0 else raw.find(body)))
    return out


def nospace_map(lines, i1, i2):
    text, pos = [], []
    for li, body, col in payloads_of(lines, i1, i2):
        for k, ch in enumerate(body):
            if ch not in SPACE:
                text.append(ch)
                pos.append((li, col + k))
    return "".join(text), pos


def is_cjk(c):
    for lo, hi in ((0x3000, 0x303F), (0x3400, 0x9FFF), (0xFF00, 0xFFEF)):
        if lo <= ord(c) <= hi:
            return True
    return c in "—…·“”‘’"


def is_word(c):
    return c.isascii() and (c.isalnum() or c == "`")


def nospace(s):
    return "".join(c for c in s if c not in SPACE)


# [junc] 交界检查：相邻两个旧行正文在新文本里是否紧贴（同行且列相邻）
def check_junctions(old_payloads, new_ns, new_map, rel, rawlines):
    out = []
    anchors = []
    for li, body, _ in old_payloads:
        ns = nospace(body)
        if len(ns) < MIN_ANCHOR:
            anchors.append(None)
            continue
        idx = new_ns.find(ns[:MIN_ANCHOR])
        anchors.append(idx if idx >= 0 and new_ns.find(ns[:MIN_ANCHOR], idx + 1) < 0 else None)
    for k in range(len(old_payloads) - 1):
        a, b = anchors[k], anchors[k + 1]
        if a is None or b is None:
            continue
        a_body, b_body = old_payloads[k][1], old_payloads[k + 1][1]
        a_ns, b_ns = nospace(a_body), nospace(b_body)
        if not a_ns or not b_ns:
            continue
        # 旧行结尾的句读不进「内容」，它就是要找的那个分隔符
        core = a_ns.rstrip("".join(SEP_PUNCT))
        sep_run = a_ns[len(core) :]
        if not core:
            continue
        a_end = a + len(core) - 1
        if a_end >= len(new_ns):
            continue
        pos_a, pos_b = new_map[a_end], new_map[b]
        if pos_a[0] != pos_b[0] or pos_b[1] != pos_a[1] + 1:
            continue  # 不在同一行相邻 → 不是粘结
        tail = a_body.rstrip()
        head = b_body.lstrip()
        sep = sep_run
        lc, rc = new_ns[a_end], new_ns[b]
        edge = ("CJK" if is_cjk(lc) else ("word" if is_word(lc) else "other")) + "/" + (
            "CJK" if is_cjk(rc) else ("word" if is_word(rc) else "other")
        )
        if sep:
            sug = f"补 {sep!r}"
        elif (is_cjk(lc) and is_word(rc)) or (is_cjk(rc) and is_word(lc)) or (is_word(lc) and is_word(rc)):
            sug = "补 ' '（词边界）"
        else:
            sug = "换行点，多半不用改"
        out.append(
            (
                rel,
                pos_b[0] + 1,
                f"交界 {edge}",
                sug,
                (tail[-24:] if tail else "") + "‖" + head[:24],
                rawlines[pos_b[0]].rstrip(),
            )
        )
    return out


# [drop] 行内标点丢失：拿旧行正文去新文本里找锚点，局部对齐后取只含标点的删除段
def check_drops(old_payloads, new_ns, new_map, rel, rawlines):
    out = []
    for li, body, _ in old_payloads:
        ns = nospace(body)
        if len(ns) < MIN_ANCHOR:
            continue
        anchor = ns[:MIN_ANCHOR]
        idx = new_ns.find(anchor)
        if idx < 0 or new_ns.find(anchor, idx + 1) >= 0:
            continue
        end = min(len(new_ns), idx + len(ns) + 16)
        seg = new_ns[idx:end]
        sm = difflib.SequenceMatcher(None, ns, seg, autojunk=False)
        if sm.ratio() < DROP_RATIO:
            continue
        for tag, a1, a2, b1, b2 in sm.get_opcodes():
            if tag not in ("delete", "replace"):
                continue
            gone = ns[a1:a2]
            if not gone or any(c not in PUNCT for c in gone):
                continue
            if not any(c in SEP_PUNCT for c in gone):
                continue
            if a2 == len(ns):
                continue  # 行末标点：交界检查已覆盖，这里只管行内
            if idx + b1 >= len(new_ns):
                continue
            pos = new_map[idx + b1]
            before = new_ns[max(0, idx + b1 - 20) : idx + b1]
            after = new_ns[idx + b1 : idx + b1 + 20]
            out.append(
                (
                    rel,
                    pos[0] + 1,
                    f"行内丢{gone}",
                    f"补 {gone!r}",
                    before + "‖" + after,
                    rawlines[pos[0]].rstrip(),
                )
            )
    return out


# [audit] 复核「工作区相对 HEAD 插进来的字符」：逐行局部对齐，标出可疑插入点
CODEY = (
    "let ",
    "fn ",
    "const ",
    "struct ",
    "enum ",
    "impl ",
    "pub ",
    "if ",
    "match ",
    "use ",
    "for ",
    "while ",
    "return",
    "#[",
)


def looks_code(payload):
    body = payload.strip()
    if any(body.startswith(k) for k in CODEY):
        return True
    if "::" in body or "->" in body:
        return True
    return False


def baseline_ns(rel, base, cache={}):
    """基线（重排前）全文的「去标记去空白」文本，用来验插入的字符当时是否真在那儿。"""
    key = (rel, base)
    if key not in cache:
        lines, _ = read_pair(rel, base)
        if lines is None:
            cache[key] = ""
        else:
            body = []
            for ln in lines:
                b = strip_marker(ln)
                body.append(nospace(b if b is not None else ln.strip()))
            cache[key] = "".join(body)
    return cache[key]


def audit(files, base="e1a27ab"):
    out = []
    for rel in files:
        old_lines, new_lines = read_pair(rel, "HEAD")
        if old_lines is None:
            continue
        base_ns = baseline_ns(rel, base)
        sm = difflib.SequenceMatcher(None, old_lines, new_lines, autojunk=False)
        for tag, i1, i2, j1, j2 in sm.get_opcodes():
            if tag == "equal" or (i2 - i1) != (j2 - j1):
                continue
            for k in range(i2 - i1):
                o, n = old_lines[i1 + k], new_lines[j1 + k]
                if o == n:
                    continue
                s = difflib.SequenceMatcher(None, o, n, autojunk=False)
                body = strip_marker(n)
                start_col = len(n.rstrip("\r\n")) - len(n.rstrip("\r\n").lstrip())
                for t, a1, a2, b1, b2 in s.get_opcodes():
                    if t != "insert" or b2 == b1:
                        continue
                    ins = n[b1:b2]
                    inside = "?"
                    if body is None:
                        body = n.strip()
                    bc = n.find(body, start_col) if body else -1
                    if bc >= 0 and b1 == bc:
                        inside = "行首(标记后)"
                    elif looks_code(body):
                        inside = "代码行内"
                    elif "\n" in ins or all(c in SPACE for c in ins):
                        inside = "空白"
                    left = n[b1 - 1] if b1 > 0 else ""
                    right = n[b2] if b2 < len(n) else ""
                    if is_word(left) and is_word(right):
                        inside = "词内"
                    if not any(c in SEP_PUNCT or c in "，（）「」：；、" or c in SPACE for c in ins):
                        inside = "非句读符号"
                    ctx = n[max(0, b1 - 26) : b1].rstrip("\r\n") + "‖" + ins + "‖" + n[b2 : b2 + 26].rstrip("\r\n")
                    needle = nospace(n[max(0, b1 - 30) : b1])[-8:] + nospace(ins) + nospace(n[b2 : b2 + 30])[:8]
                    ok = needle in base_ns if len(needle) >= 12 else False
                    if not ok and inside in ("?", "空白"):
                        inside = "基线里找不到(可疑)"
                    elif ok:
                        inside += "/基线有"
                    out.append((rel, i1 + k + 1, repr(ins), inside, ctx, n.rstrip()))
    return out


# [cli]
def main(argv=None):
    ap = argparse.ArgumentParser(description="注释折行重排的粘结定位（只读）")
    ap.add_argument("mode", choices=("list", "audit"))
    ap.add_argument("--base", default="HEAD")
    ap.add_argument("--exclude", nargs="*", default=["devimpbin/"])
    ap.add_argument("--expr", default=None)
    ap.add_argument("--out", default=None)
    a = ap.parse_args(argv)

    args = ["diff", "--name-only", "--diff-filter=M", a.base]
    if a.expr:
        args += ["--", a.expr]
    files = [
        p
        for p in run_git(args).decode("utf-8", "replace").splitlines()
        if p and not any(p.startswith(x) for x in a.exclude)
    ]

    if a.mode == "audit":
        rows = audit(files)
        buf = io.StringIO()
        kinds = {}
        for rel, line_no, ins, inside, ctx, raw in rows:
            kinds[inside] = kinds.get(inside, 0) + 1
            buf.write(f"{rel}:{line_no} ins={ins} [{inside}] {ctx}\n")
        buf.write(f"\n文件 {len(files)} 个，插入 {len(rows)} 处：{kinds}\n")
        text = buf.getvalue()
        if a.out:
            (Path(__file__).resolve().parent.parent / a.out).write_text(text, encoding="utf-8")
            sys.stdout.write(f"审计已写入 {a.out}（{len(rows)} 处 {kinds}）\n")
        else:
            sys.stdout.write(text)
        return 0

    findings = []
    for rel in files:
        old_lines, new_lines = read_pair(rel, a.base)
        if old_lines is None:
            continue
        flags_old = comment_flags(old_lines)
        flags_new = comment_flags(new_lines)
        sm = difflib.SequenceMatcher(None, old_lines, new_lines, autojunk=False)
        for tag, i1, i2, j1, j2 in sm.get_opcodes():
            if tag == "equal" or (i2 - i1) > MAX_BLOCK or (j2 - j1) > MAX_BLOCK:
                continue
            if not (all(flags_old[i1:i2]) and all(flags_new[j1:j2])):
                continue
            old_payloads = payloads_of(old_lines, i1, i2)
            new_ns, new_map = nospace_map(new_lines, j1, j2)
            if not old_payloads or not new_ns:
                continue
            findings += check_junctions(old_payloads, new_ns, new_map, rel, new_lines)
            findings += check_drops(old_payloads, new_ns, new_map, rel, new_lines)

    findings.sort(key=lambda f: (f[0], f[1]))
    buf = io.StringIO()
    kinds = {}
    for rel, line_no, kind, sug, ctx, raw in findings:
        kinds[sug] = kinds.get(sug, 0) + 1
        buf.write(f"{rel}:{line_no} [{kind}] {sug}  {ctx}\n    RAW {raw}\n")
    buf.write(f"\n文件 {len(files)} 个，候选 {len(findings)} 处：{kinds}\n")
    text = buf.getvalue()
    if a.out:
        (Path(__file__).resolve().parent.parent / a.out).write_text(text, encoding="utf-8")
        sys.stdout.write(f"候选已写入 {a.out}（{len(findings)} 处）\n")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
