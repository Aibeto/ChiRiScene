#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""devreview.py —— 代码审查辅助：改动 diff 剔注释 + 定位行号（ChiRi 协作工具，纯标准库）。

区块索引: [const] [io] [strip] [codediff] [locate] [cli]

用途：审查改动里**真正动了算法逻辑**的部分（注释增删不进 diff），以及把审查结论定位到行。
只读，不改工作区任何文件。

用法:
    python scripts\\devreview.py codediff [--staged] [--code-only] [--out FILE]
    python scripts\\devreview.py locate   <文件> [关键字...]

口径:
- `codediff` 默认比 HEAD vs 工作区（含未跟踪），`--staged` 比 HEAD vs 暂存区；
  `.rs/.ts/.svelte/.mjs/.css/.py/.sh` 先剥注释再去空行，`.yaml/.ftl/.prop/.chr/.json`
  原样比（字段序与缩进有语义）。
- 剥离走状态机（`strip_code`）：不会把字符串里的 `//`、`#` 当注释切掉。已知盲区——
  非 py 分支只认 `"`，Rust 的 char 字面量 `'"'` 会让状态机进字符串、其后注释不再被剥
  （`src/down.rs:47` 即此形状）；这类文件用 `--code-only` 滤掉注释类增删行。
- 结果自己写 UTF-8 文件，stdout 只打摘要 + 路径：PowerShell 重定向会按控制台代码页
  重编码 stdout，中文必乱码。
"""

import argparse
import difflib
import io
import os
import re
import subprocess
import sys

# [const]
CODE_EXTS = ('.rs', '.ts', '.svelte', '.mjs', '.css', '.py', '.sh')
# 结构化配置：不剥注释，原样比对（字段序/缩进都有语义）
PLAIN_EXTS = ('.yaml', '.ftl', '.prop', '.chr', '.json')
# 行首注释标记：`--code-only` 用它滤掉注释类增删行
CMT_ONLY = re.compile(r'^\s*(//|/\*|\*|#|<!--|-->|-->)')


# [io]
def repo_root():
    """脚本位于 <root>/scripts/，返回仓库根。"""
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def read(path):
    return io.open(path, encoding='utf-8', errors='replace').read()


def write_report(path, lines):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with io.open(path, 'w', encoding='utf-8', newline='\n') as f:
        if lines:
            f.write('\n'.join(lines))
            f.write('\n')
    return path


def setup_console():
    try:
        sys.stdout.reconfigure(encoding='utf-8', errors='replace')
        sys.stderr.reconfigure(encoding='utf-8', errors='replace')
    except Exception:
        pass


def announce(title, summary, path=None):
    print('== %s%s' % (title, (' -> ' + path) if path else ''))
    for s in summary:
        print('   %s' % s)


def git(root, *args):
    r = subprocess.run(['git', *args], cwd=root, capture_output=True)
    return r.stdout.decode('utf-8', 'replace')


# [strip]
def strip_code(text, ext):
    """状态机剥离注释：行注释 / 块注释 / HTML 注释，字符串内容原样保留。

    比行级正则可靠：不会把字符串里的 `//`、`#` 当注释切掉。

    已知盲区：非 py 分支只认 `"`，**不认 char 字面量**——Rust 的 `c == '"'`（表示双引号
    字符）会让状态机从这里进入字符串，此后全文注释都不再被剥离（`src/down.rs:47` 即此
    形状）；TS 正则字面量同理。审查这类文件时靠 `--code-only` 补救。
    """
    out = []
    i, n = 0, len(text)
    state = 'code'
    q = None
    py = ext in ('.py', '.sh')
    while i < n:
        c = text[i]
        nx = text[i + 1] if i + 1 < n else ''
        if state == 'code':
            if q:
                out.append(c)
                if c == '\\' and i + 1 < n:
                    out.append(text[i + 1])
                    i += 2
                    continue
                if c == q:
                    q = None
                i += 1
                continue
            if py and c in '"\'':
                q = c
                out.append(c)
                i += 1
                continue
            if not py and c == '"':
                q = c
                out.append(c)
                i += 1
                continue
            if ext in ('.ts', '.svelte', '.mjs') and c in '\'`':
                q = c
                out.append(c)
                i += 1
                continue
            if py and c == '#':
                state = 'line'
                i += 1
                continue
            if not py and c == '/' and nx == '/':
                state = 'line'
                i += 2
                continue
            if not py and c == '/' and nx == '*':
                state = 'block'
                i += 2
                continue
            if ext == '.svelte' and text.startswith('<!--', i):
                state = 'html'
                i += 4
                continue
            out.append(c)
            i += 1
            continue
        if state == 'line':
            if c == '\n':
                state = 'code'
                out.append(c)
            i += 1
            continue
        if state == 'block':
            if c == '*' and nx == '/':
                state = 'code'
                i += 2
                continue
            if c == '\n':
                out.append(c)
            i += 1
            continue
        if state == 'html':
            if text.startswith('-->', i):
                state = 'code'
                i += 3
                continue
            if c == '\n':
                out.append(c)
            i += 1
            continue
    return ''.join(out)


def norm_code(text, ext):
    """剥注释后去空行（`.rs` 的 `#[...]` 属性行保留，它不是注释）。"""
    out = []
    for l in strip_code(text, ext).split('\n'):
        s = l.strip()
        if not s:
            continue
        out.append(l.rstrip())
    return '\n'.join(out)


# [codediff]
def cmd_codediff(a):
    root = repo_root()
    if a.staged:
        files = git(root, 'diff', '--staged', '--name-only').split('\n')
    else:
        files = git(root, 'diff', '--name-only').split('\n') + \
            git(root, 'ls-files', '--others', '--exclude-standard').split('\n')
    out = []
    for f in sorted({x.strip() for x in files if x.strip()}):
        ext = os.path.splitext(f)[1]
        p = os.path.join(root, f)
        if not os.path.isfile(p) or 'devimpbin/' in f:
            continue
        head = git(root, 'show', 'HEAD:' + f)
        cur = git(root, 'show', ':' + f) if a.staged else read(p)
        if ext in CODE_EXTS:
            x, y = norm_code(head, ext), norm_code(cur, ext)
        elif ext in PLAIN_EXTS:
            x, y = head, cur
        else:
            continue
        if x == y:
            continue
        out.append('')
        for l in difflib.unified_diff(x.split('\n'), y.split('\n'),
                                      fromfile='HEAD/' + f,
                                      tofile=('INDEX/' if a.staged else 'WORK/') + f,
                                      lineterm='', n=3):
            if a.code_only and l[:1] in '-+' and not l.startswith(('---', '+++')):
                if CMT_ONLY.match(l[1:]):
                    continue
                out.append(l[:1] + l[1:].rstrip()[:150])
            else:
                out.append(l)
    path = write_report(a.out or os.path.join(root, 'devimpbin', 'code_diff.txt'), out)
    tag = ('（暂存区）' if a.staged else '') + ('（仅代码行）' if a.code_only else '')
    announce('devreview codediff%s' % tag, ['diff 行数 %d' % len(out)], path)
    return 0


# [locate]
def cmd_locate(a):
    setup_console()
    root = repo_root()
    p = a.file if os.path.isabs(a.file) else os.path.join(root, a.file)
    if not os.path.isfile(p):
        print('[!] 文件不存在: %s' % p)
        return 1
    hits = 0
    for i, l in enumerate(read(p).split('\n'), 1):
        if any(x in l for x in a.patterns):
            hits += 1
            print('%-22s:%-5d %s' % (os.path.basename(p), i, l.rstrip()[:110]))
    print('-- %d hits' % hits)
    return 0


# [cli]
def build_parser():
    ap = argparse.ArgumentParser(prog='devreview.py', description='代码审查辅助（纯标准库，只读）')
    sub = ap.add_subparsers(dest='cmd', required=True)

    p = sub.add_parser('codediff', help='改动 diff 剔注释（只看真正动了代码的部分）')
    p.add_argument('--staged', action='store_true', help='看已暂存改动（HEAD vs 暂存区）')
    p.add_argument('--code-only', action='store_true', help='再滤掉注释类增删行')
    p.add_argument('--out', default=None)
    p.set_defaults(func=cmd_codediff)

    p = sub.add_parser('locate', help='打印含关键字的行号')
    p.add_argument('file')
    p.add_argument('patterns', nargs='+')
    p.set_defaults(func=cmd_locate)
    return ap


def main(argv=None):
    setup_console()
    a = build_parser().parse_args(argv)
    return a.func(a)


if __name__ == '__main__':
    sys.exit(main())
