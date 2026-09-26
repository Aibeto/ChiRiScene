---
name: 功耗削减计划
overview: 按已有数据定位三个功耗杠杆：热限幅豁免带重构（37 分钟热窗口满频是最大头）、触摸地板降档（touch 期决策下限 1.65GHz）、全包功耗画像先行（确认哪根烟最粗，避免瞎调）。
todos:
  - id: power-profile
    content: 派整理代理产出 power-profile.md（cap85/touch/高频占用三个切片 + P_avg 排名）
    status: pending
  - id: thermal-clamp-heavy
    content: cpu_load_governor.rs 热限幅改为恒钳写侧 + feature.yaml/config.rs 加 clamp_heavy 开关
    status: pending
  - id: touch-floor-lower
    content: 8550 feature.yaml touch_boost_tiers 3→2（按画像数据定幅度，标【临时待 A/B】）
    status: pending
  - id: verify-backfill
    content: cargo check 自证，回填 device-todo.md T8 与 memory 日志
    status: pending
isProject: false
---

# 功耗削减计划

## 第 1 步：功耗画像先行（数据整理代理，只整理不分析）

现有数字只有场景级基线（bili 2.78W / QQ 尖峰 6.3W / MOBA 4.3W），不知道钱花在哪。派一个整理代理对 `devimpbin/0926-162821/` 产出 `power-profile.md`：

- 各 main\_\*.log 的 P_avg 排名 + status.csv 口径交叉验证（batt_i 整数化坑，铁律 5）；
- 三个切片的功耗对比：**cap85 窗口 vs 非 cap85**、**touch=1 vs touch=0**、**高频占用率**（tick 决策值 ≥2GHz 的时长占比，按簇）；
- top 50 功耗 tick 的构成快照（util/freq/簇）。
  这些数字直接决定第 2、3 步的参数幅度。

## 第 2 步：热限幅豁免带重构（最大杠杆，改代码）

现状（[src/chiri/cpu_load_governor.rs:544-558](src/chiri/cpu_load_governor.rs)）：`current_perf >= free_above(95)` 的簇**完全不钳制**——本包 37 分钟 cap85 窗口内高载簇决策满频烧电，这正是「点烟器」主嫌疑。设计初衷是防「决策状态卡死在 cap」，但卡死的根因是**回写 current_perf**，不是钳写频本身。

重构：cap 窗口内对**所有簇**钳写频目标，`current_perf` 照常平滑不回写（卡死根因不存在）：

```rust
// 现状：>= free_above 豁免
let eff_perf = if self.cluster.current_perf < free_above {
    self.cluster.current_perf.min(cap)
} else {
    self.cluster.current_perf
};
// 改为：恒钳写侧（current_perf 状态照常平滑，窗口解除立即恢复全速）
let eff_perf = self.cluster.current_perf.min(cap);
```

- `feature.yaml` Thermal 段加开关 `clamp_heavy: true`（serde default），想回旧行为改 false，[src/chiri/config.rs](src/chiri/config.rs) thermal 段透传；
- `free_above` 字段保留（旧语义开关关闭时使用），不删配置兼容性；
- 代价：热窗口内高负载场景性能可见下降——这就是降功耗的代价，收益由第 1 步数据量化；
- 顺带把 `clamp_change` 的 smax 读回变成「钳制生效」的行为级证据（钳制后 smax 应 ≤ cap 对应档位）。

## 第 3 步：触摸地板降档（改配置，留 A/B 验证）

数据支撑：touch=1 共 2255 行，决策下限 1651200（big 簇），来自 [module/config/8550/feature.yaml](module/config/8550/feature.yaml) `perf_init: 0.35 + tiers 3 × 0.05 = 0.50` ≈ 1.65GHz。

- `touch_boost_tiers: 3 → 2`（floor 0.50 → 0.45 ≈ 1.4GHz），或按第 1 步 touch 切片功耗占比决定幅度；
- 保留文件内既有 TODO 注释口径，标注【临时待 A/B】：滑动/打字流畅度实测后再定稿，不行就回 3 档。

## 第 4 步：收尾

- `cargo check -p chiri --target aarch64-linux-android`（Checking chiri + 0 warning）；
- `device-todo.md` T8 行更新为「重构已落地，待真机 A/B 验证功耗与性能」；
- memory 日志追加本轮改动。

顺序：1 → 2/3 并行 → 4。第 2 步是主菜；第 1 步的数据同时用来检验第 2 步收益和校准第 3 步幅度。
