// main.ts: [bootstrap] [ksu-chrome]
import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'
import { enableEdgeToEdge, fullScreen, hasKsu } from '@/kernel/ksu'
import { installMockKsu, installMockShell } from '@/dev/mock-shell'

// 无 KernelSU 注入（浏览器 dev / 自动化走查）时用设备替身，保证四屏可真实走查
// 先装 shell 再装只读 KSU 包名映射：后装不会改变已注入的 mock shell
if (!hasKsu()) {
  installMockShell()
  installMockKsu()
}

// [ksu-chrome] WebView 外层镶边：enableEdgeToEdge / fullScreen 老版本可能缺失，wrapper 内已做能力探测，调用失败不影响渲染
enableEdgeToEdge(true)
fullScreen(false)

// [bootstrap]
const target = document.getElementById('app')
if (target) {
  mount(App, { target })
}
