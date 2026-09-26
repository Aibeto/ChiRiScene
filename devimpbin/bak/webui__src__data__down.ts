// down.ts: [parse]
// DOWN 停摆状态解析（down.chr），口径必须与守护进程 src/down.rs::read_down 一致，否则「界面说停摆、调度还在跑」：
// 写了保留字 down（忽略大小写、容忍成对引号）= 停摆；空文件/只有注释/其它内容 = 正常。内容即状态，重启设备后仍停摆。

export const DOWN_WORD = 'down'

/** down.chr 内容是否表示停摆 */
export function parseDown(text: string): boolean {
  const first = text
    .split('\n')
    .map(line => line.trim())
    .find(line => line.length > 0 && !line.startsWith('#'))
  if (first === undefined) return false
  // 与 rust trim_matches 同口径：剥首尾所有引号字符（不要求成对同型），剥完不 trim 内侧——daemon 也不 trim，这里多 trim 就会裂成口径不一致。
  const bare = first.replace(/^["']+/, '').replace(/["']+$/, '')
  return bare.toLowerCase() === DOWN_WORD
}

/** 写入内容：开启写保留字 down，解除写空内容（与「没启用时只有注释」一致） */
export function downFileContent(active: boolean): string {
  return active ? `${DOWN_WORD}\n` : ''
}
