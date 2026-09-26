# devimp 分析报告

- 输入: `E:\code\ChiRi\devimpbin\logd_0925-194326.tar.gz`
- 生成: 2026-09-25 19:46:21
- 阶段: extract, analyze, main, aff, status
- 参数: --since 0000-000000 --min-n 30

## 1. extract ✅
- 输出目录 E:\code\ChiRi\devimpbin\0925-194326
- 文件 357，devimp 6 / logd 6

## 2. analyze ✅（scripts/devimp-analyze.py）
- 批次目录 6 个
- == 版本指纹 module:ChiRi Canary Alpha06-16 (versionCode 10616) | soc:8550 board=kalama model=PHB110 | android:16 (sdk 36) kernel=5.15.180-android13-8-o-01178-gfc2079499f31
- == E:\code\ChiRi\devimpbin\0925-194326\x_devimp_0925-112150  files=22  schema=44列新(拆分版)  last_ts=11:21:44.238
- mode      package                        n  P_avg   p50    p95 battT  cpuT  cap   gpu   psi    mig |          little             big           prime
- playback  tv.danmaku.bili              893   3.18  2.92   4.74  40.7  55.3   88  12.7  18.9  10991 |  47.9/ 55.2 u0.50  54.9/ 70.9 u0.59  30.8/ 50.0 u0.30
- default   me.weishu.kernelsu           111   3.83  2.94   8.13  40.4  56.9   93  15.6  19.2  11467 |  56.9/100.0 u0.57  59.3/ 82.9 u0.55  29.9/ 81.3 u0.19
- default   com.android.launcher          59   3.70  3.01   7.79  36.8  48.9   96  15.9  26.8   7686 |  47.7/ 88.6 u0.48  32.4/ 54.8 u0.29  24.2/ 27.1 u0.14

## 3. main（dvmain.py）✅
- == dvmain -> E:\code\ChiRi\devimpbin\0925-194326\main.txt
- 定版: ChiRi Canary Alpha06-16 (versionCode 10616) | 8550 / PHB110 | ['44列新(拆分版)']
- 文件 320 个；批次 6 个

## 4. aff（dvaff.py）✅
- == dvaff -> E:\code\ChiRi\devimpbin\0925-194326\aff.txt
- aff_0925-065503.log: @S 7029 帧, final_bound=136, peak=466 (单调=False), @A 行 12621, bulkΣ=91202
- aff_0925-112150.log: @S 8280 帧, final_bound=329, peak=599 (单调=False), @A 行 17640, bulkΣ=118676
- aff_0925-144208.log: @S 4520 帧, final_bound=85, peak=433 (单调=False), @A 行 17922, bulkΣ=73728
- aff_0925-155742.log: @S 5274 帧, final_bound=99, peak=548 (单调=False), @A 行 20712, bulkΣ=83763
- aff_0925-172909.log: @S 6050 帧, final_bound=133, peak=487 (单调=False), @A 行 11488, bulkΣ=97643
- aff_0925-191526.log: @S 1436 帧, final_bound=29, peak=198 (单调=False), @A 行 2035, bulkΣ=23919

## 5. status（dvstatus.py）✅
- == dvstatus -> E:\code\ChiRi\devimpbin\0925-194326\status.txt
- daemon.log 6 个, status.csv 6 个
- status 行 32589, charge={'discharging': 27468, 'charging': 5121}, fps 非空 0

## 单独重跑任一阶段

```
# 解压（重建 0925-194326/）
python scripts/devimp/dvextract.py devimpbin/logd_0925-194326.tar.gz --tag 0925-194326
# 聚合表（原脚本）
python scripts/devimp-analyze.py devimpbin/0925-194326 --since 0000-000000 --min-n 30
# 三个探针
python scripts/devimp/dvmain.py devimpbin/0925-194326 --since 0000-000000 --min-n 30
python scripts/devimp/dvaff.py devimpbin/0925-194326 --since 0000-000000
python scripts/devimp/dvstatus.py devimpbin/0925-194326
# 或一次性全跑
python scripts/devimp/dvrun.py devimpbin/logd_0925-194326.tar.gz --tag 0925-194326
```

## 失败阶段

- 无，全部阶段成功

