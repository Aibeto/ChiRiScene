#!/usr/bin/env node
// verify.mjs: 一键验收 — 风格闸门 + svelte-check + vitest + vite build，汇总退出码
import { spawnSync } from 'node:child_process'
const steps = [
  // 风格闸门排最前：风格漂移是本轮重构的头号风险，且失败信息比编译器报错更好读
  ['stylecheck', ['node', 'scripts/stylecheck.mjs']],
  ['svelte-check', ['svelte-check', '--tsconfig', './tsconfig.json', '--tsgo']],
  ['vitest', ['vitest', 'run', '--reporter=dot']],
  ['build', ['vite', 'build']],
]
let failed = 0
for (const [name, cmd] of steps) {
  console.log(`\n=== ${name} ===`)
  const r = spawnSync('npx', cmd, { stdio: 'inherit', shell: false })
  if (r.status !== 0) { console.log(`✗ ${name} exit=${r.status}`); failed++ } else console.log(`✓ ${name}`)
}
process.exit(failed ? 1 : 0)
