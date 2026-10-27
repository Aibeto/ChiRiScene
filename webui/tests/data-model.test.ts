// data-model.test.ts: [whitelists] [rules] [module-prop] [mode] [apps]
import { describe, expect, it } from 'vitest'
import { parseFasWhitelist, parseSpecialTuned, specialModeSet } from '@/data/whitelists'
import { buildRuleGroups, filterRuleGroups, parseRules, splitRuleKey } from '@/data/rules'
import { parseModuleProp } from '@/data/module-info'
import { CLG_MODE_IDS, clgFamilyKey, describeMode } from '@/data/mode'
import { buildAppEntries, filterApps } from '@/data/apps'
import { APP_LABELS } from '@/data/app-labels'

describe('白名单解析', () => {
  it('special_tuned.yaml 每行「包名:模式列表:回退模式」', () => {
    const text = [
      '# 注释行',
      'com.hypergryph.arknights:akmode:akmode',
      'com.example.multi:reduce,akmode:reduce',
      'com.example.nofallback:akmode'
    ].join('\n')
    const map = parseSpecialTuned(text)
    expect(map.get('com.hypergryph.arknights')).toEqual({ modes: ['akmode'], fallback: 'akmode' })
    expect(map.get('com.example.multi')?.modes).toEqual(['reduce', 'akmode'])
  // 缺回退模式时取模式列表首项（与 daemon 导出语义一致）
    expect(map.get('com.example.nofallback')?.fallback).toBe('akmode')
    expect(map.has('# 注释行')).toBe(false)
  })

  it('特调模式并集用于判定 current_mode 是否为特调', () => {
    const map = parseSpecialTuned('com.a:akmode:akmode\ncom.b:x,y:x\n')
    expect([...specialModeSet(map)].sort()).toEqual(['akmode', 'x', 'y'])
  })

  it('fas_whitelist.yaml 每行「包名:配置名」', () => {
    const map = parseFasWhitelist('# c\ncom.miHoYo.Yuanshen:endfield\ncom.tencent.tmgp.sgame:pubg\n')
    expect(map.get('com.miHoYo.Yuanshen')).toBe('endfield')
    expect(map.size).toBe(2)
  })

  it('残缺行被忽略', () => {
    expect(parseFasWhitelist('nocolon\n:empty\npkg:\n').size).toBe(0)
    expect(parseSpecialTuned('pkg:\n:pkg:pkg\n').size).toBe(0)
  })
})

describe('rules.yaml 解析', () => {
  it('读取全部只读展示字段', () => {
    const info = parseRules(
      [
        'dynamic_enabled: true',
        'global_mode: "default"',
        'app_modes:',
        '  com.tencent.tmgp.sgame: boost',
        'ignored_apps:',
        '  - com.android.systemui'
      ].join('\n')
    )
    expect(info.ok).toBe(true)
    expect(info.globalMode).toBe('default')
    expect(info.appModes['com.tencent.tmgp.sgame']).toBe('boost')
    expect(info.ignoredApps).toEqual(['com.android.systemui'])
  })

  it('缺字段时按守护进程默认值（dynamic_enabled/global_mode 均为真值）', () => {
    const info = parseRules('app_modes: {}\n')
    expect(info.ok).toBe(true)
    expect(info.dynamicEnabled).toBe(true)
    expect(info.globalMode).toBe('default')
  })

  it('非法 YAML 返回 ok=false 并带原因', () => {
    const info = parseRules('[[[')
    expect(info.ok).toBe(false)
    expect(info.problem).toBeTruthy()
  })

  it('activity_aware 缺省 true，显式 false 时保留', () => {
    expect(parseRules('app_modes: {}\n').activityAware).toBe(true)
    expect(parseRules('activity_aware: false\napp_modes: {}\n').activityAware).toBe(false)
  })

  it('规则键拆分：包名级 / activity 级（与 daemon 精确查表键一致）', () => {
    expect(splitRuleKey('com.tencent.mm')).toEqual({ pkg: 'com.tencent.mm', activity: '' })
    expect(splitRuleKey('com.tencent.mm/.Chat')).toEqual({
      pkg: 'com.tencent.mm',
      activity: '.Chat'
    })
  })
})

describe('规则分组（精确到 activity）', () => {
  const info = parseRules(
    [
      'dynamic_enabled: true',
      'global_mode: "default"',
      'activity_aware: true',
      'app_modes:',
      '  com.tencent.mm: reduce',
      '  com.tencent.mm/.Chat: boost',
      '  com.tencent.mm/com.tencent.mm.ui.LauncherUI: default',
      '  tv.danmaku.bili/.MainActivityV2: default',
      'ignored_apps:',
      '  - com.tencent.mm'
    ].join('\n')
  )
  const groups = buildRuleGroups(
    info,
    new Map([['com.tencent.tmgp.sgame', { modes: ['sgame'], fallback: 'sgame' }]]),
    new Map([['tv.danmaku.bili', 'bili']])
  )

  it('包名级与 activity 级归到同一分组，activity 按名排序', () => {
    const mm = groups.find(g => g.pkg === 'com.tencent.mm')
    expect(mm?.mode).toBe('reduce')
    expect(mm?.activities.map(a => a.activity)).toEqual([
      '.Chat',
      'com.tencent.mm.ui.LauncherUI'
    ])
    expect(mm?.activities[0].mode).toBe('boost')
    expect(mm?.ignored).toBe('com.tencent.mm')
  })

  it('特调 / FAS 白名单并进同一分组；只有 activity 规则的包也成组', () => {
    expect(groups.find(g => g.pkg === 'com.tencent.tmgp.sgame')?.special?.fallback).toBe(
      'sgame'
    )
    const bili = groups.find(g => g.pkg === 'tv.danmaku.bili')
    expect(bili?.fasConfig).toBe('bili')
    expect(bili?.mode).toBe('')
    expect(bili?.activities).toHaveLength(1)
  })

  it('分组按包名排序', () => {
    const names = groups.map(g => g.pkg)
    expect(names).toEqual([...names].sort())
  })

  it('搜索命中 activity / 模式 / 特调 / FAS；空白关键词不过滤', () => {
    expect(filterRuleGroups(groups, '.chat').map(g => g.pkg)).toEqual(['com.tencent.mm'])
    expect(filterRuleGroups(groups, 'bili').map(g => g.pkg)).toEqual(['tv.danmaku.bili'])
    expect(filterRuleGroups(groups, 'sgame').map(g => g.pkg)).toEqual([
      'com.tencent.tmgp.sgame'
    ])
    expect(filterRuleGroups(groups, '  ')).toHaveLength(groups.length)
  })

  it('内置应用名表覆盖当前全部规则包', () => {
    const pkgs = [
      'com.YoStarJP.Arknights',
      'com.hypergryph.arknights',
      'com.hypergryph.endfield',
      'com.levelinfinite.sgameGlobal',
      'com.salt.music',
      'com.taobao.qianniu',
      'com.taobao.taobao',
      'com.tencent.lolm',
      'com.tencent.mobileqq',
      'com.tencent.tmgp.dfm',
      'com.tencent.tmgp.sgame',
      'com.tencent.tmgp.speedmobile',
      'com.xingin.xhs',
      'tv.danmaku.bili'
    ]
    for (const pkg of pkgs) expect(APP_LABELS[pkg]).toBeTruthy()
  })

  it('搜索支持应用名（label），且不影响包名/activity 原有命中', () => {
    const labels = { 'com.tencent.mm': '微信', 'tv.danmaku.bili': '哔哩哔哩' }
    expect(filterRuleGroups(groups, '微信', labels).map(g => g.pkg)).toEqual([
      'com.tencent.mm'
    ])
    expect(filterRuleGroups(groups, '哔哩', labels).map(g => g.pkg)).toEqual([
      'tv.danmaku.bili'
    ])
    // 没有 label 的包仍按包名命中
    expect(filterRuleGroups(groups, 'sgame', labels).map(g => g.pkg)).toEqual([
      'com.tencent.tmgp.sgame'
    ])
  })
})

describe('module.prop 解析', () => {
  it('KEY=VALUE 逐行解析', () => {
    const prop = parseModuleProp(
      ['id=chiri', 'name=ChiRi', 'version=Alpha02', 'versionCode=2', 'author=Aibeto', '# c'].join('\n')
    )
    expect(prop).toEqual({
      id: 'chiri',
      name: 'ChiRi',
      version: 'Alpha02',
      versionCode: '2',
      author: 'Aibeto',
      description: ''
    })
  })
})

describe('模式派生', () => {
  const special = new Set(['akmode'])

  it('CLG 三档识别为 clg；vector 归 rhine 家族（lab）', () => {
    expect(describeMode('reduce', special).kind).toBe('clg')
    expect(describeMode('default', special).kind).toBe('clg')
    expect(describeMode('boost', special).kind).toBe('clg')
    expect(describeMode('vector', special).kind).toBe('lab')
  })

  it('contingency/babel 归 rhine 家族（lab）', () => {
    expect(describeMode('contingency', special).kind).toBe('lab')
    expect(describeMode('babel', special).kind).toBe('lab')
  })

  it('fas 与特调模式分别归类（特调需命中所见模式集合）', () => {
    expect(describeMode('fas', special).kind).toBe('fas')
    expect(describeMode('akmode', special).kind).toBe('special')
  // 正则条目的特调模式 UI 不可见 → 归为未知，不猜成特调
    expect(describeMode('secretmode', special).kind).toBe('unknown')
  })

  it('空值与未注册值都归为 unknown 且不抛错', () => {
    expect(describeMode('', special).kind).toBe('unknown')
    expect(describeMode('turbo', special).kind).toBe('unknown')
  })

  it('四档信号色互相可区分', () => {
    const signals = CLG_MODE_IDS.map(id => describeMode(id).signal)
    expect(new Set(signals).size).toBe(signals.length)
  })

  it('CLG 家族名随帕累托前沿落点切换（pf.chr = 1 → CLG-PF），家族归属不变', () => {
    expect(clgFamilyKey(true)).toBe('mode.family.clgpf')
    expect(clgFamilyKey(false)).toBe('mode.family.clg')
    // PF 只是 CLG 内部的落点机制：ModeKind 仍是 clg（按 kind 分支的地方不受影响）
    expect(describeMode('default', special).kind).toBe('clg')
  })
})

describe('应用标签合成与过滤', () => {
  const apps = [
    { pkg: 'com.hypergryph.arknights', label: '明日方舟' },
    { pkg: 'com.miHoYo.Yuanshen', label: '原神' },
    { pkg: 'com.tencent.mm', label: '微信' }
  ]
  const ctx = {
    specialTuned: parseSpecialTuned('com.hypergryph.arknights:akmode:akmode\n'),
    fasWhitelist: parseFasWhitelist('com.miHoYo.Yuanshen:endfield\n'),
    appModes: { 'com.tencent.mm': 'default' }
  }

  it('只对命中的包打标签，未命中的不臆造', () => {
    const entries = buildAppEntries(apps, ctx)
    expect(entries[0].special?.fallback).toBe('akmode')
    expect(entries[0].fasConfig).toBeUndefined()
    expect(entries[1].fasConfig).toBe('endfield')
    expect(entries[1].special).toBeUndefined()
    expect(entries[2].appMode).toBe('default')
  })

  it('按包名或应用名即时过滤', () => {
    const entries = buildAppEntries(apps, ctx)
    expect(filterApps(entries, '微信').map(e => e.pkg)).toEqual(['com.tencent.mm'])
    expect(filterApps(entries, 'MIHOYO').map(e => e.pkg)).toEqual(['com.miHoYo.Yuanshen'])
    expect(filterApps(entries, '  ')).toHaveLength(3)
  })

  it('activity 级 app_modes 键（pkg/.Activity）也给宿主包打模式标签（2026-10-24 第二批规则键形态）', () => {
    const actCtx = {
      specialTuned: new Map(),
      fasWhitelist: new Map(),
      appModes: {
        'com.tencent.mm/.plugin.voip.ui.VideoActivity': 'reduce',
        'com.coolapk.market/.view.base.SimpleActivity': 'reduce'
      }
    }
    const entries = buildAppEntries([{ pkg: 'com.tencent.mm', label: '微信' }], actCtx)
    expect(entries[0].appMode).toBe('reduce')
  })

  it('包名级优先于 activity 级键（与 daemon determine_mode 的 fallback 链同序）', () => {
    const bothCtx = {
      specialTuned: new Map(),
      fasWhitelist: new Map(),
      appModes: {
        'com.tencent.mm': 'default',
        'com.tencent.mm/.plugin.voip.ui.VideoActivity': 'reduce'
      }
    }
    const entries = buildAppEntries([{ pkg: 'com.tencent.mm', label: '微信' }], bothCtx)
    expect(entries[0].appMode).toBe('default')
  })
})
