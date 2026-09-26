# devimp 分析报告

- 输入: `E:\code\ChiRi\devimpbin\logd_0925-143934.tar.gz`
- 生成: 2026-09-25 19:47:45
- 阶段: extract, analyze, main, aff, status
- 参数: --since 0000-000000 --min-n 30

## 1. extract ✅
- 输出目录 E:\code\ChiRi\devimpbin\0925-143934
- 文件 94，devimp 2 / logd 2

## 2. analyze ✅（scripts/devimp-analyze.py）
- 批次目录 2 个
- == 版本指纹 module:ChiRi Canary Alpha06-16 (versionCode 10616) | soc:8550 board=kalama model=2210132C | android:17 (sdk 37) kernel=5.15.194-android13-8-00019-gf4321180a397-ab15212794
- == E:\code\ChiRi\devimpbin\0925-143934\x_devimp_0925-132525  files=9  schema=44列新(拆分版)  last_ts=13:25:24.739
- mode      package                        n  P_avg   p50    p95 battT  cpuT  cap   gpu   psi    mig |          little             big           prime
- default   com.tencent.mm                82   3.22  2.83   5.42  31.5  56.6  100   7.7  28.2  13295 |  63.5/ 88.6 u0.63  58.9/ 96.6 u0.49  39.0/ 92.8 u0.35
- default   com.coolapk.market            56   2.72  2.48   4.72  30.4  50.5  100  21.8  19.0  10605 |  53.7/ 82.9 u0.54  46.1/ 58.9 u0.33  26.6/ 50.0 u0.18
- --- tgtop 线程占比（近全时段） ---

## 3. main（dvmain.py）✅
- == dvmain -> E:\code\ChiRi\devimpbin\0925-143934\main.txt
- 定版: ChiRi Canary Alpha06-16 (versionCode 10616) | 8550 / 2210132C | ['44列新(拆分版)']
- 文件 78 个；批次 2 个

## 4. aff（dvaff.py）✅
- == dvaff -> E:\code\ChiRi\devimpbin\0925-143934\aff.txt
- aff_0925-131942.log: @S 341 帧, final_bound=152, peak=209 (单调=False), @A 行 3187, bulkΣ=5301
- aff_0925-132526.log: @S 4441 帧, final_bound=439, peak=912 (单调=False), @A 行 26053, bulkΣ=76455

## 5. status（dvstatus.py）✅
- == dvstatus -> E:\code\ChiRi\devimpbin\0925-143934\status.txt
- daemon.log 2 个, status.csv 2 个
- status 行 4782, charge={'discharging': 4782}, fps 非空 0

## 单独重跑任一阶段

```
# 解压（重建 0925-143934/）
python scripts/devimp/dvextract.py devimpbin/logd_0925-143934.tar.gz --tag 0925-143934
# 聚合表（原脚本）
python scripts/devimp-analyze.py devimpbin/0925-143934 --since 0000-000000 --min-n 30
# 三个探针
python scripts/devimp/dvmain.py devimpbin/0925-143934 --since 0000-000000 --min-n 30
python scripts/devimp/dvaff.py devimpbin/0925-143934 --since 0000-000000
python scripts/devimp/dvstatus.py devimpbin/0925-143934
# 或一次性全跑
python scripts/devimp/dvrun.py devimpbin/logd_0925-143934.tar.gz --tag 0925-143934
```

## 失败阶段

- 无，全部阶段成功

