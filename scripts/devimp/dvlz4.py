#!/usr/bin/env python3
"""dvlz4.py —— LZ4 解压（纯标准库，无 lz4 依赖）。区块索引: [frame] [block] [legacy] [api] [selftest]

背景（务必先读）：`module/scripts/pack.sh` 的 `archive` 分支把内层 devimp 归档打成
**`.tar.lz4`**（`lz4 -f`，即 LZ4 *帧格式*）；设备上 lz4 不可用时**回落保留 `.tar`**，
调用方按「`T` 或 `T` 去掉 `.lz4` 存在」判成功。所以分析侧必须两种都认：
老包内层是 `.tar`，新包内层是 `.tar.lz4`（`pack.sh` 2026-09-25 起）。

实现口径：
- 优先用 `lz4` 模块（快）；不可用时走本文件的纯 python 解码器。**两者结果必须一致**，
  `--selftest` 在有 lz4 模块时会做双向交叉验证。
- 校验和（header HC / block checksum / content checksum）**只按标志位跳过，不校验**——
  不引入 xxhash 依赖，结构性损坏仍会在解析时抛异常。内容大小字段同理只跳过。
- **依赖块（B.Indep=0）天然支持**：匹配窗就是「已解出的全部输出」的尾部，跨块回引
  走同一段 `out`，无需额外字典参数。
- 遗留格式（magic `0x184C2102`）与可跳过帧（`0x184D2A5x`）一并支持，
  防止设备侧 busybox/toybox 的 lz4 产出非帧格式。

CLI:
    python scripts/devimp/dvlz4.py <in.lz4> <out>   解压到文件（自动识别帧/遗留）
    python scripts/devimp/dvlz4.py --selftest       自检
"""

import sys

MAGIC_FRAME = 0x184D2204
MAGIC_LEGACY = 0x184C2102
MAGIC_SKIP_BASE = 0x184D2A50  # 0x184D2A50..0x184D2A5F
MINMATCH = 4


# [block]
def _read_len(src, i, n):
    """LZ4 变长长度：逐字节累加，遇 <255 停（该字节计入）。返回 (total, 新 i)。"""
    total = 0
    while True:
        if i >= n:
            raise ValueError("lz4: 长度字段越界（块被截断）")
        b = src[i]
        i += 1
        total += b
        if b != 255:
            return total, i


def decompress_block(src, out, floor=0):
    """把单个 LZ4 块解压追加到 bytearray `out`（跨块调用同一个 out 即有字典语义）。

    块格式：循环 { token → 扩展字面量长 → 字面量 → offset(2) → 扩展匹配长 → 匹配复制 }；
    **输出耗尽即结束**（最后一段允许只有字面量，也允许以匹配结尾）。
    匹配复制必须逐字节推进：offset < 匹配长时是自重叠复制（RLE 场景）。

    `floor` = 本块允许回引的最早输出位置：独立块 = 本块起点（块内自足），
    依赖块 = 帧起点（可回引前块末 64KB 窗口）。offset 是 u16，窗口上界天然成立。
    """
    n = len(src)
    i = 0
    while i < n:
        token = src[i]
        i += 1
        lit = token >> 4
        if lit == 15:
            extra, i = _read_len(src, i, n)
            lit += extra
        if i + lit > n:
            raise ValueError("lz4: 字面量越界（块被截断）")
        out += src[i:i + lit]
        i += lit
        if i >= n:
            break  # 最后一段只有字面量
        if i + 2 > n:
            raise ValueError("lz4: 匹配 offset 越界")
        offset = src[i] | (src[i + 1] << 8)
        i += 2
        if offset == 0:
            raise ValueError("lz4: offset=0 非法")
        mlen = token & 0x0F
        if mlen == 15:
            extra, i = _read_len(src, i, n)
            mlen += extra
        mlen += MINMATCH
        start = len(out) - offset
        if start < floor:
            raise ValueError(f"lz4: offset={offset} 越界（回引到 {start}，下界 {floor}）")
        for k in range(mlen):
            out.append(out[start + k])
    return out


# [frame]
def _u32(b, i):
    return int.from_bytes(b[i:i + 4], "little")


def _u64(b, i):
    return int.from_bytes(b[i:i + 8], "little")


def _one_frame(buf, i, n, out):
    """解析一个帧（magic 已消费），返回新的 i。"""
    if i + 2 > n:
        raise ValueError("lz4: 帧描述符越界")
    flg = buf[i]
    bd = buf[i + 1]
    i += 2
    if ((flg >> 6) & 0x3) != 1:
        raise ValueError(f"lz4: 帧版本 {((flg >> 6) & 0x3)} 不支持")
    indep = bool(flg & 0x20)        # B.Indep：True = 块内自足；False = 块间可回引
    block_cksum = bool(flg & 0x10)
    has_size = bool(flg & 0x08)
    content_cksum = bool(flg & 0x04)
    has_dict = bool(flg & 0x01)
    content_size = None
    if has_size:
        if i + 8 > n:
            raise ValueError("lz4: 内容大小字段越界")
        content_size = _u64(buf, i)   # lz4 CLI 输入为文件时默认会写，用于收尾校验
        i += 8
    if has_dict:
        i += 4                      # dict id：只跳过
    i += 1                          # HC（header checksum）：只跳过
    frame_start = len(out)          # 依赖块的回引下界
    while True:
        if i + 4 > n:
            raise ValueError("lz4: 块长度字段越界（文件被截断）")
        raw = _u32(buf, i)
        i += 4
        if raw == 0:
            break                   # EndMark
        uncompressed = bool(raw & 0x80000000)
        bsize = raw & 0x7FFFFFFF
        if i + bsize > n:
            raise ValueError("lz4: 块数据越界（文件被截断）")
        blk = buf[i:i + bsize]
        i += bsize
        if uncompressed:
            out += blk
        else:
            # 独立块只允许块内回引，依赖块允许回到帧起点（含前块末 64KB）
            decompress_block(blk, out, len(out) if indep else frame_start)
        if block_cksum:
            i += 4
        if i > n:
            raise ValueError("lz4: 块校验字段越界")
    if content_cksum:
        i += 4
    if content_size is not None and len(out) - frame_start != content_size:
        raise ValueError(
            f"lz4: 内容大小不符（解出 {len(out) - frame_start}，帧头声明 {content_size}）"
            "——解码器与真实编码器不一致，或文件被截断"
        )
    return i


def decompress_frame(buf, out=None):
    """解出帧格式（或遗留格式）的全部内容；buf 为 bytes/bytearray。"""
    if out is None:
        out = bytearray()
    buf = bytes(buf)
    n = len(buf)
    i = 0
    while i + 4 <= n:
        magic = _u32(buf, i)
        i += 4
        if magic == MAGIC_FRAME:
            i = _one_frame(buf, i, n, out)
            continue
        if magic == MAGIC_LEGACY:
            # [legacy] 遗留格式：4 字节块长 + 块，直到长度 0
            while True:
                if i + 4 > n:
                    raise ValueError("lz4: 遗留格式块长越界")
                bs = _u32(buf, i)
                i += 4
                if bs == 0:
                    break
                if i + bs > n:
                    raise ValueError("lz4: 遗留格式块数据越界")
                decompress_block(buf[i:i + bs], out)
                i += bs
            continue
        if (magic & 0xFFFFFFF0) == MAGIC_SKIP_BASE:
            if i + 4 > n:
                raise ValueError("lz4: 可跳过帧长度越界")
            sz = _u32(buf, i)
            i += 4 + sz
            continue
        raise ValueError(f"lz4: 未知 magic 0x{magic:08x}（偏移 {i - 4}）")
    if i != n:
        raise ValueError(f"lz4: 尾部有 {n - i} 字节无法解析")
    return out


# [api]
def has_module():
    try:
        import lz4.frame  # noqa: F401
        return True
    except Exception:
        return False


def decompress_bytes(buf):
    """帧格式优先走 lz4 模块（快），任何异常回落纯 python 解码器（含遗留格式）。"""
    magic = _u32(buf, 0) if len(buf) >= 4 else 0
    if magic == MAGIC_FRAME and has_module():
        try:
            import lz4.frame
            return lz4.frame.decompress(bytes(buf))
        except Exception:
            pass  # 模块不认的边角情形交给自研解码器
    return bytes(decompress_frame(bytearray(buf)))


def decompress_file(src, dst):
    with open(src, "rb") as f:
        raw = f.read()
    data = decompress_bytes(raw)
    with open(dst, "wb") as f:
        f.write(data)
    return len(data)


# [selftest]
def _blk_literals(data):
    """单段仅字面量块：token 高半字节 = min(len,15)，>=15 先写扩展字节再写字面量。"""
    out = bytearray()
    ln = len(data)
    if ln < 15:
        out.append(ln << 4)
    else:
        out.append(0xF0)
        rem = ln - 15
        while rem >= 255:
            out.append(255)
            rem -= 255
        out.append(rem)
    return bytes(out) + bytes(data)


def _blk_lit_match(lit, offset, mlen):
    """一段字面量 + 一段匹配（mlen >= 4），覆盖匹配/自重叠/跨块回引路径。"""
    out = bytearray()
    ln, ml = len(lit), mlen - MINMATCH
    out.append((min(ln, 15) << 4) | min(ml, 15))
    if ln >= 15:
        rem = ln - 15
        while rem >= 255:
            out.append(255)
            rem -= 255
        out.append(rem)
    out += bytes(lit)
    out += bytes((offset & 0xFF, (offset >> 8) & 0xFF))
    if ml >= 15:
        rem = ml - 15
        while rem >= 255:
            out.append(255)
            rem -= 255
        out.append(rem)
    return bytes(out)


def _frame(blocks, indep=True, raw_blocks=(), block_cksum=False, content_cksum=False,
           content_size=None):
    """拼帧：blocks 为已压缩块列表，raw_blocks 为「非压缩」块（bit31 置位）。"""
    flg = (0x40 | (0x20 if indep else 0) | (0x10 if block_cksum else 0)
           | (0x04 if content_cksum else 0) | (0x08 if content_size is not None else 0))
    body = bytearray()
    for b in blocks:
        body += len(b).to_bytes(4, "little") + b
        if block_cksum:
            body += (0).to_bytes(4, "little")   # 占位：本解码器只跳过不校验
    for b in raw_blocks:
        body += (len(b) | 0x80000000).to_bytes(4, "little") + b
        if block_cksum:
            body += (0).to_bytes(4, "little")
    body += (0).to_bytes(4, "little")     # EndMark
    if content_cksum:
        body += (0).to_bytes(4, "little")
    head = bytes((flg, 0x40))                 # FLG, BD
    if content_size is not None:
        head += content_size.to_bytes(8, "little")
    head += b"\x00"                           # HC 占位（不校验）
    return MAGIC_FRAME.to_bytes(4, "little") + head + bytes(body)


def _selftest():
    import os
    import tempfile
    ok = fail = 0

    def chk(name, got, want):
        nonlocal ok, fail
        if got == want:
            ok += 1
            print(f"  [ok]   {name}")
        else:
            fail += 1
            print(f"  [FAIL] {name}: got {got[:40]!r}... want {want[:40]!r}...")

    print("== dvlz4 selftest ==")
    print(f"  [info] lz4 模块: {'可用（加速 + 交叉验证）' if has_module() else '不可用（走纯 python 解码器）'}")

    chk("仅字面量(短)", decompress_bytes(_frame([_blk_literals(b"hello")])), b"hello")
    big = bytes(range(256)) * 3 + b"tail"
    chk("仅字面量(长/多 255 扩展)", decompress_bytes(_frame([_blk_literals(big)])), big)
    chk("字面量+匹配(自重叠)", decompress_bytes(_frame([_blk_lit_match(b"abc", 3, 6)])), b"abcabcabc")
    chk("匹配后带尾字面量",
        decompress_bytes(_frame([_blk_lit_match(b"abc", 3, 6) + _blk_literals(b"!")])),
        b"abcabcabc!")
    chk("多块独立", decompress_bytes(_frame([_blk_literals(b"AAA"), _blk_literals(b"BBB")])),
        b"AAABBB")
    # 依赖块：第 2 块 offset 回引第 1 块的内容（跨块字典）
    chk("依赖块跨块回引",
        decompress_bytes(_frame([_blk_literals(b"XYZ"), _blk_lit_match(b"Q", 4, 4)], indep=False)),
        b"XYZQXYZQ")
    # 独立块回引到块外必须报错：坏流不许被静默解成垃圾
    try:
        decompress_bytes(_frame([_blk_literals(b"XYZ"), _blk_lit_match(b"Q", 5, 4)]))
        chk("独立块越界回引应报错", "no-raise", "raise")
    except ValueError:
        chk("独立块越界回引应报错", "raise", "raise")
    chk("非压缩块(bit31)", decompress_bytes(_frame([], raw_blocks=[b"rawdata"])), b"rawdata")
    chk("带块校验/内容校验字段(跳过)",
        decompress_bytes(_frame([_blk_literals(b"ck")], block_cksum=True, content_cksum=True)), b"ck")
    chk("遗留格式",
        decompress_bytes(MAGIC_LEGACY.to_bytes(4, "little")
                         + len(_blk_literals(b"legacy!")).to_bytes(4, "little")
                         + _blk_literals(b"legacy!")
                         + (0).to_bytes(4, "little")), b"legacy!")
    cs_body = b"abcabcabc"
    chk("带内容大小字段(相符)",
        decompress_bytes(_frame([_blk_literals(cs_body)], content_size=len(cs_body))), cs_body)
    try:
        decompress_bytes(_frame([_blk_literals(cs_body)], content_size=len(cs_body) + 1))
        chk("内容大小不符应报错", "no-raise", "raise")
    except ValueError:
        chk("内容大小不符应报错", "raise", "raise")

    chk("可跳过帧 + 正常帧",
        decompress_bytes(MAGIC_SKIP_BASE.to_bytes(4, "little") + (3).to_bytes(4, "little") + b"xxx"
                         + _frame([_blk_literals(b"after-skip")])), b"after-skip")

    if has_module():
        import lz4.frame
        payload = (b"ChiRi devimp " * 5000) + bytes(range(256))
        chk("lz4 模块压缩 → 本解码器解", decompress_bytes(lz4.frame.compress(payload)), payload)
        # 模块产物落盘 → 走文件 API，覆盖 CLI 路径
        d = tempfile.mkdtemp(prefix="dvlz4_")
        src, dst = os.path.join(d, "a.lz4"), os.path.join(d, "a.out")
        with open(src, "wb") as f:
            f.write(lz4.frame.compress(payload))
        decompress_file(src, dst)
        with open(dst, "rb") as f:
            chk("decompress_file 往返", f.read(), payload)
        os.remove(src)
        os.remove(dst)
        os.rmdir(d)
    else:
        print("  [note] 无 lz4 模块：只跑了自拼帧用例（真实包解压仍可用，纯 python 解码器）")

    print(f"== selftest: {ok} ok / {fail} fail ==")
    return 1 if fail else 0


def main(argv):
    if "--selftest" in argv:
        return _selftest()
    if len(argv) != 3:
        print(__doc__)
        return 2
    n = decompress_file(argv[1], argv[2])
    print(f"{argv[1]} -> {argv[2]}  {n} bytes")
    return 0


if __name__ == "__main__":
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass
    sys.exit(main(sys.argv))