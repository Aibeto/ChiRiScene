# P4 implementation approval

主线已批准 P4 最小保真方案：纯模块缓存同身份完整六槽序列化结果，按槽位短片 yield_now；build 超过 200ms 时改为本帧 full 锚点，补读原本跳过的冷长尾，不以陈旧缓存冒充新采样；最终 p mask 阶段之后复核慢阈值。候选集合稳定冻结、原子写整帧、所有字段和采样质量保留、worker 默认 false 不改。允许实施与 RED/GREEN、独立审查；实机无 adb 留 TODO。

说明：send_message 对当前 subagent id 返回 active teammate not found，批准落此文档供复核。
