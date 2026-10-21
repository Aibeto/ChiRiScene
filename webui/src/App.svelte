<script lang="ts">
  // App.svelte: [shell] [bar] [views] [nav] 顶层骨架：顶栏（关闭 / 模块名版本 + 守护进程状态 + 界面语言）、路由视图区、亚克力底栏
  // 场景用 Mica 底（.g-scene 中性面 + 顶部一层品牌蓝光晕 + .g-noise 颗粒），亚克力只上在顶栏/底栏/卡片这些浮层
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

  // [custom-bg] 自定义背景：与 index.html 同目录存在 backend.chr 时用它作为页面背景；'./backend.chr' 相对**文档**（index.html 所在目录）解析，不是相对打包后的 JS 目录
  // 用 Image 探测而不是 fetch/HEAD：WebView 的 file:// 或自定义 scheme 下 fetch 常被同源策略拒绝，<img> 加载不受此限且顺带校验可解码（不是图片时静默保持 Mica 底）
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

  /** 关闭 WebUI：优先走管理器注入的 ksu.exit（真正的原生关闭）
   *  拿不到该 API（旧版管理器 / 外部浏览器）时才退回 window.close() + 历史后退，只做后退会变成返回上一页 */
  function exitWebui(): void {
    if (exitApp()) return
    window.close()
    setTimeout(() => window.history.back(), 120)
  }
</script>

<!-- 场景层：Mica 底 + 全屏颗粒（.g-noise 的 ::after 固定在视口上，不随滚动位移） -->
<div class="shell g-scene g-noise {customBg ? 'g-scene--custom-bg' : ''}">
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
        <span class="g-reveal" aria-hidden="true"></span>
        ✕
      </button>
      <div class="shell__brand g-brand">
        <span class="g-icon" aria-hidden="true">CR</span>
        <div class="shell__names">
          <p class="shell__title">{app.moduleProp.name || t('app.title')}</p>
          <p class="shell__version u-mono">
            {app.moduleProp.version || t('app.subtitle')}
          </p>
        </div>
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
        <span class="g-reveal" aria-hidden="true"></span>
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
        <span class="g-reveal" aria-hidden="true"></span>
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

  /* [custom-bg] 自定义背景层：与 index.html 同目录的 backend.chr，铺满视口；position:fixed 而非 absolute——absolute 的包含块是 .g-scene，页面比视口长时背景会随内容拉长、滚动到底露出空白
     层级用 z-index:-1 落在场景底色之上、所有内容之下；不能给 0，定位元素会画在非定位的正文之上把文字整段盖住 */
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
    /* 用户图不可控可能很亮，压暗一层让亚克力卡片与暗字仍有对比；亚克力本身仍是无色半透明 */
    filter: brightness(0.88) saturate(1.02);
  }

  /* 有自定义背景时关掉 Mica 底色与光晕，背景图自己就是场景（同一时刻只应有一个场景） */
  .g-scene--custom-bg {
    background: #e9e7e5;
  }
  .g-scene--custom-bg::before {
    display: none;
  }

  /* 吸顶顶栏：强亚克力（28px 模糊）+ 1px 底边收边，滚动时内容在它下面虚化穿过
     左右内距在宽屏下自动加大到「与 720px 正文列对齐」，所以不给顶栏套 max-width */
  .shell__bar {
    position: sticky;
    top: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: calc(var(--space-2) + env(safe-area-inset-top, 0px))
      max(var(--space-4), calc((100% - 720px) / 2)) var(--space-2);
    border-bottom: var(--hairline) solid var(--surface-border);
    background: var(--acrylic-strong);
    -webkit-backdrop-filter: blur(var(--blur-strong)) saturate(120%);
    backdrop-filter: blur(var(--blur-strong)) saturate(120%);
    box-shadow: var(--shadow-1);
  }

  .shell__left {
    min-width: 0;
  }

  .shell__brand {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .shell__names {
    min-width: 0;
  }

  .shell__title {
    margin: 0;
    color: var(--text-primary);
    font-size: 0.9375rem;
    font-weight: 600;
    letter-spacing: -0.01em;
    line-height: 1.25;
  }

  .shell__version {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.625rem;
    letter-spacing: 0.06em;
    line-height: 1.4;
  }

  .shell__side {
    flex: 0 0 auto;
  }

  .lang {
    font-size: 0.75rem;
    font-weight: 700;
  }

  /* 底部留出底栏的高度 + 安全区，避免最后一张卡片被压住；宽屏居中收窄到 720px（Fluent 的阅读宽度） */
  .shell__main {
    flex: 1 1 auto;
    width: 100%;
    max-width: 720px;
    margin-inline: auto;
    padding: var(--space-4);
    padding-bottom: calc(68px + var(--space-6) + var(--safe-b));
  }

  /* 底栏：亚克力药丸浮在场景上（左右留边 + 圆角两处都是「浮起」的关键），阴影用第 3 档
     宽屏下居中并限宽 520px，避免四项被拉成四根长条 */
  .shell__nav {
    position: fixed;
    z-index: 30;
    bottom: calc(var(--space-3) + var(--safe-b));
    left: 50%;
    width: min(520px, calc(100% - 2 * var(--space-3)));
    transform: translateX(-50%);
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: var(--space-1);
    padding: var(--space-1);
    border: var(--hairline) solid var(--acrylic-border);
    border-radius: var(--radius-l);
    background: var(--acrylic-strong);
    -webkit-backdrop-filter: blur(var(--blur-strong)) saturate(120%);
    backdrop-filter: blur(var(--blur-strong)) saturate(120%);
    box-shadow: var(--shadow-3), var(--shadow-inset);
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
    color: var(--text-muted);
    font-size: 0.8125rem;
    font-weight: 600;
  }

  .shell__nav-item:hover:not(:disabled) {
    border-color: transparent;
    background: rgb(0 0 0 / 4%);
    box-shadow: none;
    color: var(--text-primary);
    transform: none;
  }

  /* 选中态：品牌蓝淡底 + 深蓝字，顶端一条 2px 蓝色指示条（Fluent 导航的选中语言，不是整条色边） */
  .shell__nav-item--active,
  .shell__nav-item--active:hover:not(:disabled) {
    border-color: transparent;
    background: var(--accent-tint);
    color: var(--accent-pressed);
    box-shadow: none;
  }

  .shell__nav-item--active::after {
    position: absolute;
    top: 5px;
    left: 50%;
    width: 18px;
    height: 2px;
    margin-left: -9px;
    border-radius: 1px;
    content: '';
    background: var(--accent);
  }

  .shell__nav-item:active:not(:disabled) {
    transform: scale(0.97);
  }
</style>
