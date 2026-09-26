# devimp 分析报告

- 输入: `E:\code\ChiRi\devimpbin\logd_0925-193233.tar.gz`
- 生成: 2026-09-25 19:47:45
- 阶段: extract, analyze, main, aff, status
- 参数: --since 0000-000000 --min-n 30

## 1. extract ✅
- 输出目录 E:\code\ChiRi\devimpbin\0925-193233
- 文件 117，devimp 2 / logd 2

## 2. analyze ✅（scripts/devimp-analyze.py）
- 批次目录 2 个
- == 版本指纹 module:ChiRi Canary Alpha06-16 (versionCode 10616) | soc:8550 board=kalama model=2210132C | android:17 (sdk 37) kernel=5.15.215-KylinKernel-260911-Dev
- == E:\code\ChiRi\devimpbin\0925-193233\x_devimp_0925-183107  files=32  schema=44列新(拆分版)  last_ts=18:31:04.449
- mode      package                        n  P_avg   p50    p95 battT  cpuT  cap   gpu   psi    mig |          little             big           prime
- default   com.ss.android.ugc.aweme.m   528   3.07  2.68   5.28  33.2  44.4  100   6.9  13.0   8544 |  28.5/ 33.3 u0.25  41.1/ 58.9 u0.31  46.9/ 73.5 u0.43
- default   com.tencent.mm                72   5.37  4.88   9.18  34.3  55.0  100  14.0  33.1  22871 |  38.5/ 61.0 u0.39  72.2/ 96.6 u0.69  84.1/100.0 u0.83
- default   com.miui.home                 56   4.45  3.97   9.42  31.3  60.9  100   9.0  26.4  10900 |  39.7/ 66.7 u0.37  35.3/ 87.7 u0.29  37.0/ 92.8 u0.28

## 3. main（dvmain.py）✅
- == dvmain -> E:\code\ChiRi\devimpbin\0925-193233\main.txt
- 定版: ChiRi Canary Alpha06-16 (versionCode 10616) | 8550 / 2210132C | ['44列新(拆分版)']
- 文件 102 个；批次 2 个

## 4. aff（dvaff.py）✅
- == dvaff -> E:\code\ChiRi\devimpbin\0925-193233\aff.txt
- aff_0925-151823.log: @S 5457 帧, final_bound=3, peak=8 (单调=False), @A 行 2093, bulkΣ=82447
- aff_0925-183107.log: @S 3071 帧, final_bound=85, peak=287 (单调=False), @A 行 5336, bulkΣ=43932

## 5. status（dvstatus.py）✅
- == dvstatus -> E:\code\ChiRi\devimpbin\0925-193233\status.txt
- daemon.log 2 个, status.csv 2 个
- status 行 8528, charge={'discharging': 7048, 'charging': 1480}, fps 非空 0

## 单独重跑任一阶段

```
# 解压（重建 0925-193233/）
python scripts/devimp/dvextract.py devimpbin/logd_0925-193233.tar.gz --tag 0925-193233
# 聚合表（原脚本）
python scripts/devimp-analyze.py devimpbin/0925-193233 --since 0000-000000 --min-n 30
# 三个探针
python scripts/devimp/dvmain.py devimpbin/0925-193233 --since 0000-000000 --min-n 30
python scripts/devimp/dvaff.py devimpbin/0925-193233 --since 0000-000000
python scripts/devimp/dvstatus.py devimpbin/0925-193233
# 或一次性全跑
python scripts/devimp/dvrun.py devimpbin/logd_0925-193233.tar.gz --tag 0925-193233
```

## 失败阶段

- 无，全部阶段成功

