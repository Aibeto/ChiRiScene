# devimp 分析报告

- 输入: `E:\code\ChiRi\devimpbin\logd_0927-003237.tar.gz`
- 生成: 2026-09-27 00:40:36
- 阶段: extract, analyze, main, aff, status
- 参数: --since 0000-000000 --min-n 30

## 1. extract ✅
- 输出目录 E:\code\ChiRi\devimpbin\0927-003237
- 文件 138，devimp 4 / logd 4

## 2. analyze ✅（scripts/devimp-analyze.py）
- 批次目录 4 个
- == 版本指纹 module:ChiRi Canary Alpha07-04 (versionCode 10704) | soc:8550 board=kalama model=PHB110 | android:16 (sdk 36) kernel=5.15.180-android13-8-o-01178-gfc2079499f31
- == E:\code\ChiRi\devimpbin\0927-003237\x_devimp_0926-212121  files=28  schema=44列新(拆分版)  last_ts=21:21:18.395
- mode      package                        n  P_avg   p50    p95 battT  cpuT  cap   gpu   psi    mig |          little             big           prime
- playback  tv.danmaku.bili             3128   3.56  3.50   5.10  40.5  56.1   91  15.7  22.0  12667 |  43.3/ 50.5 u0.53  51.0/ 66.0 u0.59  31.4/ 50.0 u0.36
- default   me.weishu.kernelsu           253   3.14  2.78   5.32  39.3  52.4  100  19.4  20.6  12341 |  44.1/ 66.7 u0.59  36.7/ 58.9 u0.38  23.1/ 27.1 u0.12
- default   com.tencent.mobileqq         136   4.04  3.17   9.26  40.9  58.8   89  19.5  27.5   9890 |  55.5/ 82.9 u0.58  54.1/ 87.7 u0.55  32.4/ 85.5 u0.28

## 3. main（dvmain.py）✅
- == dvmain -> E:\code\ChiRi\devimpbin\0927-003237\main.txt
- 定版: ChiRi Canary Alpha07-04 (versionCode 10704) | 8550 / PHB110 | ['44列新(拆分版)']
- 文件 113 个；批次 4 个

## 4. aff（dvaff.py）✅
- == dvaff -> E:\code\ChiRi\devimpbin\0927-003237\aff.txt
- aff_0926-195323.log: @S 5162 帧, final_bound=1121, peak=1149 (单调=False), @A 行 48080, bulkΣ=72561
- aff_0926-212121.log: @S 4463 帧, final_bound=1065, peak=1153 (单调=False), @A 行 46036, bulkΣ=70965
- aff_0926-223635.log: @S 5220 帧, final_bound=623, peak=720 (单调=False), @A 行 33631, bulkΣ=65198
- aff_0927-000711.log: @S 1443 帧, final_bound=22, peak=342 (单调=False), @A 行 6346, bulkΣ=18313

## 5. status（dvstatus.py）✅
- == dvstatus -> E:\code\ChiRi\devimpbin\0927-003237\status.txt
- daemon.log 4 个, status.csv 4 个
- status 行 16288, charge={'discharging': 13704, 'charging': 2584}, fps 非空 0

## 单独重跑任一阶段

```
# 解压（重建 0927-003237/）
python scripts/devimp/dvextract.py devimpbin\logd_0927-003237.tar.gz --tag 0927-003237
# 聚合表（原脚本）
python scripts/devimp-analyze.py devimpbin/0927-003237 --since 0000-000000 --min-n 30
# 三个探针
python scripts/devimp/dvmain.py devimpbin/0927-003237 --since 0000-000000 --min-n 30
python scripts/devimp/dvaff.py devimpbin/0927-003237 --since 0000-000000
python scripts/devimp/dvstatus.py devimpbin/0927-003237
# 或一次性全跑
python scripts/devimp/dvrun.py devimpbin\logd_0927-003237.tar.gz --tag 0927-003237
```

## 失败阶段

- 无，全部阶段成功

