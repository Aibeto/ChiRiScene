<script lang="ts">
  // App.svelte: [shell] [bar] [views] [nav]顶层骨架：顶栏（关闭 / 模块名版本 + 守护进程状态 + 界面语言）、路由视图区、玻璃底栏
  // 场景（深墨夜景纯色底 + 噪点，2026-10-16 起光源光斑已删）挂在 .g-scene/.g-grain 上——见 app.css [scene]
  // 光斑删除后「有东西可模糊」的前提削弱：玻璃质感由纯色底 + 颗粒噪点 + 自定义背景图承担
  import { onMount } from 'svelte'
  import LiveStatus from '@/components/LiveStatus.svelte'
  import AppsView from '@/views/AppsView.svelte'
  import BatteryView from '@/views/BatteryView.svelte'
  import ConfigView from '@/views/ConfigView.svelte'
  import AdvancedView from '@/views/AdvancedView.svelte'
  import LabView from '@/views/LabView.svelte'
  import LogsView from '@/views/LogsView.svelte'
  import OverviewView from '@/views/OverviewView.svelte'
  import { exitApp } from '@/kernel/ksu'
  import { i18n, t, toggleLocale } from '@/i18n/index.svelte'
  import { go, initRouter, navOwner, router, VIEW_IDS } from '@/router.svelte'
  import { app } from '@/state.svelte'

  const navItems = $derived(VIEW_IDS.map(id => ({ id, label: t(`nav.${id}`) })))
  // 二级视图（实验室、电池读数）挂在配置页下，导航高亮跟着父视图走
  const navActive = $derived(navOwner(router.view))
  const daemonLabel = $derived(t(`daemon.${app.daemonState}`))

  onMount(() => {
    initRouter()
    if (!app.ready) void app.loadOverview()
    probeBackendBg()
  })

  // [custom-bg] 自定义背景：与 index.html 同目录存在 backend.chr 时，用它作为页面背景。
  //
  // 为什么用 Image 探测而不是 fetch/HEAD：设备上页面跑在 WebView 的 file:// 或自定义 scheme 下，
  // fetch 常被同源策略拒绝；<img> 加载不受此限，且顺带校验它确实是浏览器能解码的图片
  // （文件存在但内容不是图片时静默保持夜景底，不出现半张破图）。
  // './backend.chr' 是相对**文档**（index.html 所在目录）解析的，不是相对打包后的 JS 目录。
  const BACKEND_BG_URL = './backend.chr'
  let customBg = $state(false)

  function probeBackendBg(): void {
    const probe = new Image()
    probe.onload = () => {
      // naturalWidth > 0 才算真正解码成功（onload 也可能在 0 尺寸时触发）
      if (probe.naturalWidth > 0) customBg = true
    }
    probe.onerror = () => {
      customBg = false
    }
    probe.src = BACKEND_BG_URL
  }

  /**
   * 关闭 WebUI：优先走管理器注入的 ksu.exit（真正的原生关闭）
   * 只有拿不到该 API（旧版管理器 / 外部浏览器）时才退回 window.close() + 历史后退——
   * 只做后退会变成「返回上一页」，不是关闭
   */
  function exitWebui(): void {
    if (exitApp()) return
    window.close()
    setTimeout(() => window.history.back(), 120)
  }
</script>

<!-- 场景层：底色 + 全屏噪点（2026-10-16 起光源光斑已删；.g-grain 的 ::after 固定在视口上，不随滚动位移） -->
<div class="shell g-scene g-grain {customBg ? 'g-scene--custom-bg' : ''}">
  {#if customBg}
    <!-- 自定义背景：与 index.html 同目录的 backend.chr（src 是运行时字符串，Vite 不参与解析，
         故相对**文档**而非打包后的 assets/ 目录解析——写成 CSS url() 会落到 assets/backend.chr 而 404） -->
    <img class="shell__bg" src={BACKEND_BG_URL} alt="" aria-hidden="true" />
  {/if}
  <header class="shell__bar">
    <div class="shell__left u-row-tight">
      <button
        type="button"
        class="g-btn g-btn--icon"
        aria-label={t('app.exit')}
        onclick={exitWebui}
      >
        <span class="g-sheen"></span>
        ✕
      </button>
      <div class="shell__brand">
        <p class="shell__title">{app.moduleProp.name || t('app.title')}</p>
        <p class="shell__version u-mono">
          {app.moduleProp.version || t('app.subtitle')}
        </p>
      </div>
    </div>
    <div class="shell__side u-row-tight">
      <LiveStatus state={app.daemonState} label={daemonLabel} />
      <button
        type="button"
        class="g-btn g-btn--icon lang"
        aria-label={t('action.toggleLocale')}
        onclick={toggleLocale}
      >
        <span class="g-sheen"></span>
        {i18n.locale === 'zh' ? 'EN' : '中文'}
      </button>
    </div>
  </header>

  <main class="shell__main">
    {#if router.view === 'overview'}
      <OverviewView />
    {:else if router.view === 'config'}
      <ConfigView />
    {:else if router.view === 'apps'}
      <AppsView />
    {:else if router.view === 'logs'}
      <LogsView />
    {:else if router.view === 'battery'}
      <BatteryView />
    {:else if router.view === 'advanced'}
      <AdvancedView />
    {:else}
      <LabView />
    {/if}
  </main>

  <nav class="shell__nav" aria-label={t('nav.main')}>
    {#each navItems as item (item.id)}
      <button
        type="button"
        class="g-btn shell__nav-item"
        class:shell__nav-item--active={navActive === item.id}
        aria-current={navActive === item.id ? 'page' : undefined}
        onclick={() => go(item.id)}
      >
        <span class="g-sheen"></span>
        {item.label}
      </button>
    {/each}
  </nav>
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    min-height: 100vh;
    min-height: 100dvh;
  }

  /* [custom-bg] 自定义背景层：与 index.html 同目录的 backend.chr，铺满视口。
     position:fixed 而非 absolute——absolute 的包含块是 .g-scene，页面比视口长时背景会随内容拉长、
     滚动到底露出空白；fixed 恒定贴合视口。层级用 z-index:-1，与 .g-scene::before（原光源光斑）同一套路：
     落在场景底色之上、所有内容之下。**不能给 0**——定位元素（含 z-index:0）会画在非定位的正文之上，
     把 .shell__main 里没套玻璃面板的文字整段盖住；-1 才是在流内容之下。 */
  .shell__bg {
    position: fixed;
    z-index: -1;
    inset: 0;
    width: 100%;
    height: 100%;
    /* 自适应铺满：cover 保证始终填满视口、允许出界裁切（绝不出现留白），且两个方向等比——
       禁止单边拉伸（100% 100% / stretch 会按宽高各自缩放、纵横比失真），故用 object-fit: cover */
    object-fit: cover;
    object-position: center;
    pointer-events: none;
    /* 用户图不可控，可能很亮：压暗一层让上方白字仍达 AA。这是"给场景提亮/压暗"，
       不是给玻璃上色——玻璃仍是无色半透明 */
    filter: brightness(0.72) saturate(1.05);
  }

  /* 有自定义背景时关掉 .g-scene 的夜景底（含已空的 ::before 光斑层；光斑 2026-10-16 已删）：
     背景图自己就是"可折射之物"（叠纯色夜景底只会盖掉背景图，风格铁律：颜色由场景承载，只能有一个场景） */
  .g-scene--custom-bg {
    background: var(--night-deep);
  }
  .g-scene--custom-bg::before {
    display: none;
  }

  /*
   * 吸顶顶栏：重玻璃（60px 模糊）。底部用 1px 收边而不是实色分隔线——
   * 玻璃之间靠"受光顶边 + 底边暗缘"分层，画线会把它变回普通导航栏
   */
  .shell__bar {
    position: sticky;
    top: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: calc(var(--space-3) + env(safe-area-inset-top, 0px)) var(--space-4) var(--space-3);
    border-bottom: var(--hairline) solid var(--glass-border);
    background: var(--glass-bg-weak);
    -webkit-backdrop-filter: blur(var(--glass-blur-heavy)) saturate(var(--glass-saturate));
    backdrop-filter: blur(var(--glass-blur-heavy)) saturate(var(--glass-saturate));
    box-shadow: 0 8px 32px rgb(3 7 18 / 45%), inset 0 -1px 0 rgb(2 6 16 / 30%);
  }

  .shell__left {
    min-width: 0;
  }

  .shell__brand {
    min-width: 0;
  }

  .shell__title {
    margin: 0;
    color: var(--text-primary);
    font-size: 1.0625rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    line-height: 1.3;
  }

  .shell__version {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.625rem;
    letter-spacing: 0.1em;
    line-height: 1.4;
  }

  .shell__side {
    flex: 0 0 auto;
  }

  .lang {
    font-size: 0.75rem;
    font-weight: 700;
  }

  /* 底部留出药丸底栏的高度 + 安全区，避免最后一张卡片被压住 */
  .shell__main {
    flex: 1 1 auto;
    padding: var(--space-4);
    padding-bottom: calc(64px + var(--space-6) + var(--safe-b));
  }

  /* 底栏：玻璃药丸浮在场景上（left/right 留边 + 圆角两处都是"浮起"的关键） */
  .shell__nav {
    position: fixed;
    z-index: 30;
    right: var(--space-4);
    bottom: calc(var(--space-3) + var(--safe-b));
    left: var(--space-4);
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: var(--space-1);
    padding: var(--space-1);
    border: var(--hairline) solid var(--glass-border);
    border-radius: var(--radius-l);
    background: var(--glass-bg);
    -webkit-backdrop-filter: blur(var(--glass-blur-heavy)) saturate(var(--glass-saturate));
    backdrop-filter: blur(var(--glass-blur-heavy)) saturate(var(--glass-saturate));
    box-shadow: var(--glass-shadow-lifted);
  }

  /* 覆盖 .g-btn 的圆角与最小高度：底栏项是栅格里的扁按钮，不是标准 CTA */
  .shell__nav-item {
    /* 必须显式 width:100% + min-width:0：.g-btn 是 inline-flex，栅格下不收会横向溢出 */
    width: 100%;
    min-width: 0;
    min-height: 52px;
    padding: 0 var(--space-2);
    border-color: transparent;
    border-radius: var(--radius-m);
    background: transparent;
    box-shadow: none;
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
    color: var(--text-muted);
    font-size: 0.8125rem;
    font-weight: 600;
  }

  .shell__nav-item:hover:not(:disabled) {
    border-color: transparent;
    background: var(--glass-bg-weak);
    box-shadow: none;
    transform: none;
    color: var(--text-primary);
  }

  /* 选中态：干玻璃底 + 香槟金顶边（唯一强调色用于"当前所在"这一关键状态） */
  .shell__nav-item--active,
  .shell__nav-item--active:hover:not(:disabled) {
    border-color: var(--glass-border);
    background: var(--glass-bg-strong);
    color: var(--text-primary);
    box-shadow: var(--glass-shadow-inset), inset 0 2px 0 0 var(--champagne);
  }

  .shell__nav-item:active:not(:disabled) {
    transform: scale(0.97);
  }
</style>
