# 2026-11-20 devimpbin 清理清单

## 结论
- 清理前 **28G**，清理后 **4.5G**，释放约 **23.5G**。范围严格限于项目内 `devimpbin/`（`.gitignore` 第 52 行已忽略，不进版本库）。
- 累计推理量不受影响：批次行数已进 `scripts/devimp/soc_batches.tsv` 账本（只增不减），删目录不回退，口径见 `agentsdocs/05-maint.md` 的 `[analysis]` 段。

## 保留（28 项）
- **最新一包**：`1107-011543/`（1.0G）+ `logd_1107-011543.tar`（77MB，解包源）。
- **含报告正文证据的批次**：`1031-2340/`（FINDINGS_2340.md）、`1031-kmsg/`（FINDINGS.md）、`1101-0853/`（FINDINGS_1101.md）、`a0214/`（REPORT-A02-14.md，A02-14 基线包）。
- **工具与资产**：`web/`（`cape.dtsi`、`anchors_sm8650.yaml`，`agentsdocs/06-kernel.md` 引用，gitignored 需重取）、`_xlog/`、`_x1030/`（model-audit.md、tencent-video.md、em_test）、`_tmp/`（tmp_chk_speed6.py）、`ui-shots/`。
- **构建/模型缓存**：`_hf-cache/`（560MB，bge-m3-onnx 本地副本，本机 huggingface 不可达，删了要重新下载）、`rustup-home/`（852MB）、`_cargo_home/`（70MB）。
- **脚本**：`batch_ingest.py`、`gen_soc_config.py`（`agentsdocs/02-convention.md` 引用）、`ub.sh`、`ub_offsets.py`、`ccheck.py`、`ci-lint.sh`、`hostcheck.sh`、`nightly-clippy.sh`、`selftest-build-arch.sh`、`fix-memsearch-onnx.sh`、`style-spec.md`、`fps_probe_check.md`、`fbmap`、`soc_dts_evidence.json`、`zumapro_raw.json`、`keep-me`、`p23_replay` / `p23_replay.json` / `p23_scratch.rs` / `p4_scratch.rs`。

## 删除（74 项）
```
0927-142328 0927-181339 0928-170828 0928-205518 0928-210731 0928-devA 0928-new 0928-pf
1001-045147 1001-190148 1002-042344 1002-135302 1002-143231 1003-213457 1003-214440
1003-220345 1003-230849 1004-233049 1005-021253 1005-170643 1006-123730 1006-131110
1006-180755 1011-000909 1016-001727 1016-001727-raw 1016-151808 1016-165413
1023-011754 1025-195122 1025-210433 1025-211846 1026-082752 1026-200034 1026-213008
1026-221511 1026-225313 1028-173647 1028-234751 1030-164933 1030-175147 1030-181950
1031-portal AirPortal-1793441039967.zip __pycache__ _dbcost _tmp_1107 _tmp_fmt _x1009
_x1026 _x1026b _x1026c affinity_cpuset_regression.final.log build-arch-trash
clg_tuning_regression.final.log diag_slice_regression.final.log final-check.log
logd_1023 logd_1023-011754-outer p23_replay_run.log p23_tmp review-1017 rs-top-tmp
rust_regression.final.log scheduling-accounting-tests.log scheduling-baseline-check.log
scheduling-selfcost-tests.log test-replay-final.log tmp_8650x tmp_hbbrowser
tmp_odin2_verify.log tmp_v2test tmp_webui-dev.log validation-tmp
```

其中 `0928-*`、`10xx-*` 为解包批次日录；`_x1009`（2.4G，含仅存于此的 1007 批次）、`_x1026`/`_x1026b`/`_x1026c`（394MB）、`_dbcost`（227MB，与 `_x1016` 同内容）、`tmp_8650x`（= `1003-213457/x_devimp_1002-202815` 的副本）为解包中转与重复副本；其余为一次性运行日志与空目录。

## 副作用与说明
- 文档中指向已删目录的路径（`05-maint.md`、`06-kernel.md`、`.cursor/memory/2026-10-31.md` 等）**按 AGENTS.md「文件名含日期的 md 不回改」保持原样**，其定量结论已落在正文；判读时以本文为准，这批原始日志不可再复算。
- `scripts/devimp/dvcount.py` 之后只能复算「现存可核」部分；账本 `scripts/devimp/soc_batches.tsv` 的 215 行才是累计量权威。

