// app-labels.ts: [labels] 规则包 → 展示用应用名（WebUI 内置，不读本机安装状态）
// 规则页只展示 rules.yaml / special_tuned.yaml / fas_whitelist.yaml 里出现的包；
// 应用名随模块维护，避免进页时调用 KernelSU getPackagesInfo（未安装的包也要有名字）。
// 新增规则包时在这里补一行；未收录的包由视图回落为「未知应用」+ 包名。

export const APP_LABELS: Readonly<Record<string, string>> = {
  'com.YoStarJP.Arknights': 'アークナイツ',
  'com.hypergryph.arknights': '明日方舟',
  'com.hypergryph.endfield': '明日方舟：终末地',
  'com.levelinfinite.sgameGlobal': 'Honor of Kings',
  'com.salt.music': 'Salt Player',
  'com.taobao.qianniu': '千牛',
  'com.taobao.taobao': '淘宝',
  'com.tencent.lolm': '英雄联盟手游',
  'com.tencent.mobileqq': 'QQ',
  'com.tencent.tmgp.dfm': '三角洲行动',
  'com.tencent.tmgp.sgame': '王者荣耀',
  'com.tencent.tmgp.speedmobile': 'QQ飞车',
  'com.xingin.xhs': '小红书',
  'tv.danmaku.bili': '哔哩哔哩',
  // ignored_apps 里的系统包：规则列表通常不会为它成组，留一条便于排查
  'com.android.systemui': '系统界面'
}
