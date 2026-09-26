# devimp 分析报告

- 输入: `E:\code\ChiRi\devimpbin\logd_0926-162821.tar.gz`
- 生成: 2026-09-26 16:33:35
- 阶段: extract, analyze, main, aff, status
- 参数: --since 0000-000000 --min-n 30

## 1. extract ✅
- 输出目录 E:\code\ChiRi\devimpbin\0926-162821
- 文件 53，devimp 2 / logd 2

## 2. analyze ✅（scripts/devimp-analyze.py）
- 批次目录 2 个
- == 版本指纹 module:ChiRi Canary Alpha07-02 (versionCode 10702) | soc:8550 board=kalama model=PHB110 | android:16 (sdk 36) kernel=5.15.180-android13-8-o-01178-gfc2079499f31
- == E:\code\ChiRi\devimpbin\0926-162821\x_devimp_0926-162614  files=36  schema=44列新(拆分版)  last_ts=16:26:11.728
- mode      package                        n  P_avg   p50    p95 battT  cpuT  cap   gpu   psi    mig |          little             big           prime
- playback  tv.danmaku.bili             3251   3.08  2.89   4.91  40.0  53.5   93  13.0  20.7  11477 |  43.9/ 50.5 u0.54  52.7/ 66.0 u0.61  31.4/ 50.0 u0.36
- default   com.tencent.mobileqq         243   4.83  4.19  10.02  40.3  61.6   94  22.6  28.9  12407 |  60.5/ 94.3 u0.64  62.5/ 96.6 u0.62  37.8/ 92.8 u0.36
- default   com.perol.pixez              227   2.70  2.30   5.77  41.0  49.9   85  10.7  14.2   5922 |  44.4/ 82.9 u0.50  48.9/ 68.5 u0.43  23.8/ 53.6 u0.16

## 3. main（dvmain.py）✅
- == dvmain -> E:\code\ChiRi\devimpbin\0926-162821\main.txt
- 定版: ChiRi Canary Alpha07-02 (versionCode 10702) | 8550 / PHB110 | ['44列新(拆分版)']
- 文件 40 个；批次 2 个

## 4. aff（dvaff.py）✅
- == dvaff -> E:\code\ChiRi\devimpbin\0926-162821\aff.txt
- aff_0926-150648.log: @S 4756 帧, final_bound=295, peak=674 (单调=False), @A 行 49709, bulkΣ=77094
- aff_0926-162614.log: @S 122 帧, final_bound=7, peak=7 (单调=False), @A 行 75, bulkΣ=2193

## 5. status（dvstatus.py）✅
- == dvstatus -> E:\code\ChiRi\devimpbin\0926-162821\status.txt
- daemon.log 2 个, status.csv 2 个
- status 行 4878, charge={'discharging': 4878}, fps 非空 0

## 单独重跑任一阶段

```
# 解压（重建 0926-162821/）
python scripts/devimp/dvextract.py devimpbin\logd_0926-162821.tar.gz --tag 0926-162821
# 聚合表（原脚本）
python scripts/devimp-analyze.py devimpbin/0926-162821 --since 0000-000000 --min-n 30
# 三个探针
python scripts/devimp/dvmain.py devimpbin/0926-162821 --since 0000-000000 --min-n 30
python scripts/devimp/dvaff.py devimpbin/0926-162821 --since 0000-000000
python scripts/devimp/dvstatus.py devimpbin/0926-162821
# 或一次性全跑
python scripts/devimp/dvrun.py devimpbin\logd_0926-162821.tar.gz --tag 0926-162821
```

## 失败阶段

- 无，全部阶段成功

