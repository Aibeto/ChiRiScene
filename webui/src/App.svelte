<script lang="ts">
  // App.svelte: [shell] [bar] [views] [nav] 顶层骨架（app-frame）：顶栏（关闭 / 模块名版本）、路由视图区（唯一滚动容器）、亚克力底栏
  // 界面语言入口在状态页首卡右下角（English WebUI / 中文界面），顶栏不再放语言按钮
  // 外壳视口高、页面本身不滚；顶/底栏在文档流内，因此不依赖 sticky/fixed 也不会被内容滚走
  // 场景用 Mica 底（.g-scene 中性面 + 顶部一层品牌蓝光晕 + .g-noise 颗粒），亚克力只上在顶栏/底栏/卡片这些浮层
  import { onMount } from 'svelte'
  import AppsView from '@/views/AppsView.svelte'
  import BatteryView from '@/views/BatteryView.svelte'
  import ConfigView from '@/views/ConfigView.svelte'
  import AdvancedView from '@/views/AdvancedView.svelte'
  import LabView from '@/views/LabView.svelte'
  import LogsView from '@/views/LogsView.svelte'
  import OverviewView from '@/views/OverviewView.svelte'
  import { exitApp } from '@/kernel/ksu'
  import { t } from '@/i18n/index.svelte'
  import { go, initRouter, navOwner, router, VIEW_IDS } from '@/router.svelte'
  import { app } from '@/state.svelte'

  const navItems = $derived(VIEW_IDS.map(id => ({ id, label: t(`nav.${id}`) })))
  // 二级视图（实验室、电池读数）挂在配置页下，导航高亮跟着父视图走
  const navActive = $derived(navOwner(router.view))

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
        <div class="shell__names">
          <p class="shell__title">{app.moduleProp.name || t('app.title')}</p>
          <p class="shell__version u-mono">
            {app.moduleProp.version || t('app.subtitle')}
          </p>
        </div>
      </div>
    </div>
  </header>

  <main class="shell__main">
    <div class="shell__content">
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
    </div>
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
  /* [frame] 应用外壳：视口高、页面本身不滚动，只有 .shell__main 内部滚动。
     顶栏/底栏都放在文档流里（不用 sticky/fixed）——Android WebView 里 fixed 元素叠加
     backdrop-filter 时合成层会跟着内容滚走，sticky 在部分 WebView 也不可靠；app-frame 让两条栏天然常驻 */
  .shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    height: 100dvh;
    min-height: 0;
    overflow: hidden;
    box-sizing: border-box;
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

  /* 顶栏：流内第一段（不再吸顶），亚克力材质不变；左右内距在宽屏下自动对齐 720px 正文列 */
  .shell__bar {
    position: relative;
    z-index: 20;
    flex: 0 0 auto;
    box-sizing: border-box;
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

  /* 唯一滚动容器：页面本身不滚，只有内容区滚 —— 顶/底栏因此始终留在视口内 */
  .shell__main {
    flex: 1 1 auto;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
    overscroll-behavior: contain;
    -webkit-overflow-scrolling: touch;
  }

  /* 正文列：最大 720px（Fluent 阅读宽度）居中，很横屏时不会把卡片拉成一条长线；
     border-box 是「宽度不超屏」的关键 —— content-box 下 width:100% 再叠内距会横向溢出 */
  .shell__content {
    box-sizing: border-box;
    width: 100%;
    max-width: 720px;
    margin-inline: auto;
    padding: var(--space-4);
    padding-bottom: var(--space-6);
  }

  /* 底栏：流内最后一段（不再 fixed），仍是浮起的亚克力药丸（左右留边 + 圆角）；
     宽屏下居中并限宽 520px，避免四项被拉成四根长条；border-box 保证内距不把药丸撑出屏幕 */
  .shell__nav {
    position: relative;
    z-index: 30;
    flex: 0 0 auto;
    box-sizing: border-box;
    width: min(520px, calc(100% - 2 * var(--space-3)));
    margin: 0 auto calc(var(--space-3) + var(--safe-b));
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
