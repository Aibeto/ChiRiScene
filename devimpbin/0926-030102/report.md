# devimp 分析报告

- 输入: `E:\code\ChiRi\devimpbin\logd_0926-030102.tar.gz`
- 生成: 2026-09-26 03:09:43
- 阶段: extract, analyze, main, aff, status
- 参数: --since 0000-000000 --min-n 30

## 1. extract ✅
- 输出目录 E:\code\ChiRi\devimpbin\0926-030102
- 文件 79，devimp 1 / logd 1

## 2. analyze ✅（scripts/devimp-analyze.py）
- 批次目录 1 个
- == 版本指纹 module:ChiRi Canary Alpha06-18 (versionCode 10618) | soc:8550 board=kalama model=2210132C | android:17 (sdk 37) kernel=5.15.194-android13-8-00019-gf4321180a397-ab15212794
- == E:\code\ChiRi\devimpbin\0926-030102\x_devimp_0926-030100  files=70  schema=44列新(拆分版)  last_ts=03:00:58.928
- mode      package                        n  P_avg   p50    p95 battT  cpuT  cap   gpu   psi    mig |          little             big           prime
- default   com.coolapk.market           342   2.72  2.51   4.34  28.7  52.6  100   9.1  23.3  10835 |  70.5/ 94.3 u0.68  53.3/ 68.5 u0.39  29.4/ 62.0 u0.22
- default   com.tencent.mobileqq         217   3.53  3.07   6.99  29.9  64.1  100  13.4  27.9  13152 |  72.7/ 94.3 u0.71  58.1/ 82.9 u0.49  33.9/ 85.5 u0.28
- default   com.ss.android.ugc.aweme      30   4.86  4.88   7.20  30.2  63.0  100  16.9  30.3  19637 |  79.9/100.0 u0.78  65.3/ 92.5 u0.61  45.5/ 92.8 u0.41

## 3. main（dvmain.py）✅
- == dvmain -> E:\code\ChiRi\devimpbin\0926-030102\main.txt
- 定版: ChiRi Canary Alpha06-18 (versionCode 10618) | 8550 / 2210132C | ['44列新(拆分版)']
- 文件 70 个；批次 1 个

## 4. aff（dvaff.py）✅
- == dvaff -> E:\code\ChiRi\devimpbin\0926-030102\aff.txt
- aff_0926-023555.log: @S 1502 帧, final_bound=144, peak=661 (单调=False), @A 行 8516, bulkΣ=32312

## 5. status（dvstatus.py）✅
- == dvstatus -> E:\code\ChiRi\devimpbin\0926-030102\status.txt
- daemon.log 1 个, status.csv 1 个
- status 行 1502, charge={'discharging': 796, 'charging': 706}, fps 非空 0

## 单独重跑任一阶段

```
# 解压（重建 0926-030102/）
python scripts/devimp/dvextract.py devimpbin/logd_0926-030102.tar.gz --tag 0926-030102
# 聚合表（原脚本）
python scripts/devimp-analyze.py devimpbin/0926-030102 --since 0000-000000 --min-n 30
# 三个探针
python scripts/devimp/dvmain.py devimpbin/0926-030102 --since 0000-000000 --min-n 30
python scripts/devimp/dvaff.py devimpbin/0926-030102 --since 0000-000000
python scripts/devimp/dvstatus.py devimpbin/0926-030102
# 或一次性全跑
python scripts/devimp/dvrun.py devimpbin/logd_0926-030102.tar.gz --tag 0926-030102
```

## 失败阶段

- 无，全部阶段成功

