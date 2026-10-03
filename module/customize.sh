#!/system/bin/sh
# customize.sh: [busybox] [i18n] [welcome] [hot-update-check] [volume-key] [battery-detect] [config-keep]
# [mode-select] [hot-update-flow] [full-install]

# ChiRi Scheduler 安装脚本

# [busybox] $MODPATH 为 Magisk 传入的模块安装路径

# --- 自动检测 BusyBox ---
if [ -x "/data/adb/magisk/busybox" ]; then
  BUSYBOX="/data/adb/magisk/busybox"
elif [ -x "/data/adb/ksu/bin/busybox" ]; then
  BUSYBOX="/data/adb/ksu/bin/busybox"
elif [ -x "/data/adb/ap/bin/busybox" ]; then
  BUSYBOX="/data/adb/ap/bin/busybox"
fi

# [i18n] 语言定义
CURRENT_LOCALE=$(/system/bin/getprop persist.sys.locale)
if [ -z "$CURRENT_LOCALE" ]; then
    CURRENT_LOCALE=$(/system/bin/getprop ro.product.locale)
fi

LANG_CODE="en"
MSG_WELCOME="ChiRi Scheduler"
MSG_SELECT_MODE="Please select installation mode:"
MSG_VOLUME_UP="[Volume UP] Normal installation (requires reboot)"
MSG_VOLUME_DOWN="[Volume DOWN] Hot update (Experimental)[Recommended]"
MSG_SELECTED_UP="Selected: Normal installation (Requires reboot)"
MSG_SELECTED_DOWN="Selected: Hot update (Experimental)[Recommended]"
MSG_HOT_UPDATE_START="Mission start"
MSG_STOPPING_DAEMON="Stopping daemon process..."
MSG_STOPPING_MAIN="Stopping main process..."
MSG_COPYING_FILES="Copying module files..."
MSG_RESTARTING_SERVICE="Restarting service..."
MSG_HOT_UPDATE_DONE="Mission accomplished"
MSG_FULL_INSTALL="Proceeding with normal installation"
MSG_HOT_UPDATE_UNAVAILABLE="Hot update unavailable, falling back to normal installation..."
MSG_RESTARTING_SCHEDULER="Restarting scheduler..."
MSG_VERIFY_SERVICE="Verifying daemon service..."
MSG_SERVICE_FAIL="Restart process failed, please try again or choose normal installation"
MSG_HOT_UPDATE_HINT="If you encounter any errors at the end, please ignore them. Run Action manually, or the scheduler will stop once the manager closes."
MSG_INSTALL_CANCELLED="Installation cancelled"
MSG_HOT_UPDATE_ABORT="Hot update done. Please run Action manually, or the scheduler will stop once the manager closes."
MSG_KEEP_CONFIG_ASK="Keep the existing module configuration?"
MSG_KEEP_CONFIG_UP="[Volume UP] Keep existing configuration (not recommended)"
MSG_KEEP_CONFIG_DOWN="[Volume DOWN] Overwrite the existing configuration with the defaults"
MSG_KEEP_CONFIG_YES="Existing configuration kept"
MSG_KEEP_CONFIG_NO="Replaced with the default configuration"
MSG_OPLUS_ON="OPlus private node detected; enabled private-node readings in the new config"
MSG_OPLUS_DUAL_ON="Second cell detected (index 11); enabled dual-cell voltage"
MSG_OPLUS_NO_META="Could not locate this device's meta.yaml, so the battery switches and divisors were not auto-applied (set them in the WebUI battery page)"
MSG_OPLUS_NO_SED="sed is unavailable, so the battery switches and divisors were not auto-applied (set them in the WebUI battery page)"
MSG_OPLUS_CHECK="Checking for the OPlus private node (bcc_parms)..."
MSG_OPLUS_SKIP="No OPlus private node on this device; falling back to the standard node"
MSG_VOLT_CHECK="Reading the standard voltage node to calibrate the unit divisors..."
MSG_VOLT_APPLY="Calibration divisors written (same value for voltage and current)"
MSG_VOLT_FAIL="Standard voltage node unreadable or out of the 3-4.5 V range; divisors left as they are"
MSG_SCREEN_FLIP="Screen-state property read 1 at install; screen_off_value flipped to 0 in meta.yaml"

if echo "$CURRENT_LOCALE" | $BUSYBOX grep -qi "zh"; then
  LANG_CODE="zh"
  MSG_WELCOME="ChiRi 千漓调度"
  MSG_SELECT_MODE="模式选择："
  MSG_VOLUME_UP="[ 音量 + ] 普通安装【需手动重新配置】"
  MSG_VOLUME_DOWN="[ 音量 - ] 热更新【推荐】"
  MSG_SELECTED_UP="已选择：普通安装【需手动重新配置】"
  MSG_SELECTED_DOWN="已选择：热更新【推荐】"
  MSG_HOT_UPDATE_START="任务开始"
  MSG_STOPPING_DAEMON="正在停止守护进程..."
  MSG_STOPPING_MAIN="正在停止主进程..."
  MSG_COPYING_FILES="正在复制模块文件..."
  MSG_RESTARTING_SERVICE="正在重启服务..."
  MSG_HOT_UPDATE_DONE="完成"
  MSG_FULL_INSTALL="开始安装"
  MSG_HOT_UPDATE_UNAVAILABLE="热更新不可用，回退到完整安装..."
  MSG_RESTARTING_SCHEDULER="正在重启调度器..."
  MSG_VERIFY_SERVICE="正在确认守护进程状态..."
  MSG_SERVICE_FAIL="重启进程失败，请重试或使用普通安装"
  MSG_HOT_UPDATE_HINT="如有报错请忽略。调度未启动，需要手动执行一次Action"
  MSG_INSTALL_CANCELLED="安装已取消"
  MSG_HOT_UPDATE_ABORT="热更新已完成，如有报错请忽略。调度未启动，需要手动执行一次Action"
  MSG_KEEP_CONFIG_ASK="是否保留模块内已有的配置？"
  MSG_KEEP_CONFIG_UP="[ 音量 + ] 保留现有配置（不建议）"
  MSG_KEEP_CONFIG_DOWN="[ 音量 - ] 使用默认配置覆盖现有配置"
  MSG_KEEP_CONFIG_YES="已保留"
  MSG_KEEP_CONFIG_NO="已更换为默认配置"
  MSG_OPLUS_ON="检测到 OPlus 私有节点，已为新配置启用私有节点读取"
  MSG_OPLUS_DUAL_ON="检测到第二个电芯，已启用双电芯电压"
  MSG_OPLUS_NO_META="未定位到当前机型的 meta.yaml，电池读数开关与校准倍数未自动写入（可在 WebUI 电池读数页设置）"
  MSG_OPLUS_NO_SED="sed 不可用，电池读数开关与校准倍数未自动写入（可在 WebUI 电池读数页设置）"
  MSG_OPLUS_CHECK="正在检查 OPlus 私有节点（bcc_parms）..."
  MSG_OPLUS_SKIP="本机没有 OPlus 私有节点，回退标准节点读数"
  MSG_VOLT_CHECK="正在读取标准节点电压，推算校准倍数..."
  MSG_VOLT_APPLY="已写入校准倍数（电压与电流同值）"
  MSG_VOLT_FAIL="标准节点电压读数不可用或不在 3~4.5V 量级，校准倍数保持原样，安装完成后请打开webui手动校准"
  MSG_SCREEN_FLIP="安装时屏幕状态属性读数为 1，已把 meta.yaml 的 screen_off_value 改为 0"

fi

# [welcome] 欢迎信息
ui_print " "
ui_print "$MSG_WELCOME"
ui_print " "

# [hot-update-check] 热更新可用条件：zip 内与已安装模块的 allowHotUpdate 均存在且内容为 1
ZIP_HOT_UPDATE_FLAG="$MODPATH/allowHotUpdate"
INSTALLED_HOT_UPDATE_FLAG="/data/adb/modules/chiri/allowHotUpdate"

HOT_UPDATE_AVAILABLE=false
if [ -f "$ZIP_HOT_UPDATE_FLAG" ] && [ "$(cat "$ZIP_HOT_UPDATE_FLAG")" = "1" ] && \
  [ -f "$INSTALLED_HOT_UPDATE_FLAG" ] && [ "$(cat "$INSTALLED_HOT_UPDATE_FLAG")" = "1" ]; then
    HOT_UPDATE_AVAILABLE=true
fi

# [volume-key] 音量键检测（兼容 Magisk/KernelSU 环境）：安装模式选择与「是否保留配置」两处共用；返回 0=音量上 1=音量下 2=错误（多次按下事件）
# 单次按键可能被多个输入设备重复上报，同轮内同键多次 DOWN 去重为一次按下
detect_volume_key() {
    ui_print "等待音量键按下..."
    ui_print "Waiting for volume key press..."

    # 临时文件写入模块暂存目录（/tmp 可能不可写）
    local tmp_file="$MODPATH/.getevent_output"
    rm -f "$tmp_file"

    getevent -l > "$tmp_file" 2>/dev/null &
    local getevent_pid=$!

    local round=0
    local first_key=""
    local first_round=-1
    local last_up=0
    local last_down=0

    # 每 0.1s 轮询：先等第一次按下，之后 1s 确认窗口
    while [ $round -lt 120 ]; do
        local up=$(grep -c "KEY_VOLUMEUP.*DOWN" "$tmp_file" 2>/dev/null || echo 0)
        local down=$(grep -c "KEY_VOLUMEDOWN.*DOWN" "$tmp_file" 2>/dev/null || echo 0)

        if [ -z "$first_key" ]; then
            if [ "$up" -gt 0 ]; then
                first_key="KEY_VOLUMEUP"
                first_round=$round
                last_up=$up
            elif [ "$down" -gt 0 ]; then
                first_key="KEY_VOLUMEDOWN"
                first_round=$round
                last_down=$down
            fi
        else
            # 确认窗口：1s 内无第二次按下即返回结果
            if [ $((round - first_round)) -ge 10 ]; then
                kill $getevent_pid 2>/dev/null
                rm -f "$tmp_file"
                if [ "$first_key" = "KEY_VOLUMEUP" ]; then
                    return 0
                else
                    return 1
                fi
            fi
            # 第二次按下：出现另一按键，或同键计数在新轮次增加
            if [ "$first_key" = "KEY_VOLUMEUP" ]; then
                if [ "$down" -gt 0 ] || [ "$up" -gt "$last_up" ]; then
                    kill $getevent_pid 2>/dev/null
                    rm -f "$tmp_file"
                    ui_print "错误：检测到多个音量键按下事件！"
                    ui_print "Error: Multiple volume key press events detected!"
                    return 2
                fi
            else
                if [ "$up" -gt 0 ] || [ "$down" -gt "$last_down" ]; then
                    kill $getevent_pid 2>/dev/null
                    rm -f "$tmp_file"
                    ui_print "错误：检测到多个音量键按下事件！"
                    ui_print "Error: Multiple volume key press events detected!"
                    return 2
                fi
            fi
        fi
        sleep 0.1
        round=$((round + 1))
    done

    # 超时：清理收尾（已有按下则按其返回，否则用默认选项）
    kill $getevent_pid 2>/dev/null
    rm -f "$tmp_file"
    if [ -n "$first_key" ]; then
        if [ "$first_key" = "KEY_VOLUMEUP" ]; then
            return 0
        else
            return 1
        fi
    fi
    ui_print "未检测到音量键，使用默认选项..."
    ui_print "No volume key detected, using the default choice..."
    return 0
}

# [battery-detect] 按机型把电池读数开关与校准倍数写进当前机型的 meta.yaml
# （$MODPATH=modules_update 暂存份，完整安装由安装器落地、热更新由下方 cp -r 覆盖到 live，两条路拿到的都是这份，只改一次）：
# 定位：优先 active_config.chr（daemon 写的生效配置相对路径，如 8550/meta.yaml），否则用 rosoc.model 数字部分拼机型目录；
# 都拿不到则打印说明跳过，不猜别的机型

# OPlus 私有节点 bcc_parms 可读 → oplus_chg 改 true；第 12 个字段（0 基下标 11）
# 为正 → oplus_dual_cell 改 true；
# 校准倍数统一入口 apply_divisor_from_raw（标准节点取 voltage_now，私有节点取下标 6 的电芯0电压）只在「不保留配置」时调用
# （保留时那份 meta.yaml 是用户文件，安装器不碰；这些项随时可在 WebUI 电池读数页改）定义必须早于调用点（普通安装与热更新两条路都会用到）
OPLUS_BCC="/sys/class/oplus_chg/battery/bcc_parms"
STD_VOLT="/sys/class/power_supply/battery/voltage_now"

# 定位当前机型的 meta.yaml，结果放进全局 META_FILE（定位不到就留空）
META_FILE=""
SED_CMD=""
locate_meta_file() {
    META_FILE=""
    local rel=""
    local active="/data/adb/modules/chiri/active_config.chr"
    if [ -f "$active" ]; then
        rel=$(cat "$active" 2>/dev/null | tr -d ' \t\r\n')
    fi
    if [ -z "$rel" ]; then
        local soc=$(getprop ro.soc.model 2>/dev/null | tr -cd '0-9')
        if [ -n "$soc" ] && [ -f "$MODPATH/config/$soc/meta.yaml" ]; then
            rel="$soc/meta.yaml"
        fi
    fi
    if [ -n "$rel" ] && [ -f "$MODPATH/config/$rel" ]; then
        META_FILE="$MODPATH/config/$rel"
    fi
}

# 校准倍数统一入口：电压原始值 n 位 → 除数 = 1 后跟 n-1 个 0（4382→1000、4382000→1000000，均为 4.382V），电流套用同值；首位必须为 3/4（电池 3~4.5V），
# 否则不猜、留模板缺省值让用户在 WebUI 改返回 1 = 没写
apply_divisor_from_raw() {
    local meta="$1"
    local raw
    raw=$(printf '%s' "$2" | tr -d ' \t\r\n-')
    case "$raw" in
        ''|*[!0-9]*) return 1 ;;
    esac
    case "$raw" in
        3*|4*) ;;
        *) return 1 ;;
    esac

    local len=${#raw}
    local div="1"
    local i=1
    while [ "$i" -lt "$len" ]; do
        div="${div}0"
        i=$((i + 1))
    done
    $SED_CMD -i "s/^voltage_divisor: .*/voltage_divisor: $div/" "$meta"
    $SED_CMD -i "s/^current_divisor: .*/current_divisor: $div/" "$meta"
    ui_print "$MSG_VOLT_APPLY"
    ui_print "raw=$raw -> divisor=$div"
    return 0
}

# 私有节点可用 → 开 oplus_chg / oplus_dual_cell；返回 1 = 没有私有节点」，调用方接着走标准节点校准
apply_oplus_switches() {
    local meta="$1"
    local bcc=""
    if [ -f "$OPLUS_BCC" ]; then
        bcc=$(cat "$OPLUS_BCC" 2>/dev/null)
    fi
    if [ -z "$bcc" ]; then
        ui_print "$MSG_OPLUS_SKIP"
        return 1
    fi

    if grep -q "^oplus_chg: false" "$meta"; then
        $SED_CMD -i "s/^oplus_chg: false/oplus_chg: true/" "$meta"
        ui_print "$MSG_OPLUS_ON"
    fi
    # 先数逗号定字段数：cut 在字段不足时会把整行原样透传，不先数就会拿错值——下面两处都要用
    local commas=$(printf '%s' "$bcc" | tr -cd ',')
    # 校准倍数走同一入口，原始值取私有节点电芯0电压（下标 6 = 第 7 项，故需 ≥6 个逗号）；推算不出就不写：daemon 此时也用不了该节点、会回退标准节点，与模板缺省 1000000 口径正好对上
    if [ ${#commas} -ge 6 ]; then
        apply_divisor_from_raw "$meta" "$(printf '%s' "$bcc" | cut -d ',' -f 7)"
    fi
    # 双电芯 = 第 12 个字段（0 基下标 11）存在且为正数，与 daemon read_oplus_bcc 同口径
    local f12=""
    if [ ${#commas} -ge 11 ]; then
        f12=$(printf '%s' "$bcc" | cut -d ',' -f 12 | tr -d ' \t\r\n')
    fi
    case "$f12" in
        ''|*[!0-9]*|0) ;;
        *)
            if grep -q "^oplus_dual_cell: false" "$meta"; then
                $SED_CMD -i "s/^oplus_dual_cell: false/oplus_dual_cell: true/" "$meta"
                ui_print "$MSG_OPLUS_DUAL_ON"
            fi
            ;;
    esac
    return 0
}

# 没有私有节点 → 走标准 power_supply 节点：取 voltage_now 交给统一入口推算
apply_standard_divisor() {
    local meta="$1"
    ui_print "$MSG_VOLT_CHECK"
    local raw=""
    if [ -f "$STD_VOLT" ]; then
        raw=$(cat "$STD_VOLT" 2>/dev/null)
    fi
    apply_divisor_from_raw "$meta" "$raw" || ui_print "$MSG_VOLT_FAIL"
}

apply_battery_defaults() {
    # 无论命中与否都打印：只在成功时打会让人分不清「没检测」和「检测了但没命中」
    ui_print "$MSG_OPLUS_CHECK"

    locate_meta_file
    if [ -z "$META_FILE" ]; then
        ui_print "$MSG_OPLUS_NO_META"
        return 0
    fi

    if [ -n "$BUSYBOX" ] && [ -x "$BUSYBOX" ]; then
        SED_CMD="$BUSYBOX sed"
    elif command -v sed >/dev/null 2>&1; then
        SED_CMD="sed"
    fi
    if [ -z "$SED_CMD" ]; then
        ui_print "$MSG_OPLUS_NO_SED"
        return 0
    fi

    if ! apply_oplus_switches "$META_FILE"; then
        apply_standard_divisor "$META_FILE"
    fi

    # [screen-detect] 实测 debug.tracing.screen_state=1 与模板默认「1 为息屏」相反：翻转写入 0；读到 0 或缺失不动
    local prop_val=$(getprop debug.tracing.screen_state 2>/dev/null | tr -d ' \t\r\n')
    if [ "$prop_val" = "1" ] && grep -q "^screen_off_value: 1" "$META_FILE"; then
        $SED_CMD -i "s/^screen_off_value: 1/screen_off_value: 0/" "$META_FILE"
        ui_print "$MSG_SCREEN_FLIP"
    fi
}

# [config-keep] 「是否保留现有配置」：音量上键/超时 = 保留（删掉暂存目录的 config/，完整安装落地与热更新 cp -r 都不会覆盖已有配置）；音量下键 = 不保留（不做任何事，
# 由包内模板覆盖，并走 [battery-detect]）保留不等于放任旧文件：daemon 启动与每次热重载前会调 common::sync_meta_snapshot()
# 自愈——缺键按内嵌默认补齐（meta 字段全可选），格式非法才整份覆盖（meta 的 nofix 开关跳过该自愈）config/ 里 i18n/*.ftl、normal/*.yaml 是随包副本，运行期不读，
# 保留时停在旧版本
ask_keep_config() {
    ui_print "$MSG_KEEP_CONFIG_ASK"
    ui_print "$MSG_KEEP_CONFIG_UP"
    ui_print "$MSG_KEEP_CONFIG_DOWN"
    ui_print " "

    detect_volume_key
    keep_result=$?
    if [ $keep_result -eq 2 ]; then
        ui_print "$MSG_INSTALL_CANCELLED"
        abort "$MSG_INSTALL_CANCELLED"
    fi
    if [ $keep_result -eq 0 ]; then
        ui_print "$MSG_KEEP_CONFIG_YES"
        CONFIG_KEPT=true
        rm -rf "$MODPATH/config"
    else
        ui_print "$MSG_KEEP_CONFIG_NO"
        CONFIG_KEPT=false
        # 覆盖安装：按机型把电池读数开关与校准倍数写好（两条安装路径都经过这里）
        apply_battery_defaults
    fi
    ui_print " "
}

# [mode-select] 安装模式选择（音量键交互）
if [ "$HOT_UPDATE_AVAILABLE" = "true" ]; then
    ui_print "$MSG_SELECT_MODE"
    ui_print "$MSG_VOLUME_UP"
    ui_print "$MSG_VOLUME_DOWN"
    ui_print " "
    
    # 音量键检测函数见 [volume-key] 段
    detect_volume_key
    choice_result=$?
    
    if [ $choice_result -eq 2 ]; then
        ui_print "$MSG_INSTALL_CANCELLED"
        # 用 abort 而非 exit：exit 跳过安装器收尾清理，暂存残留会让模块被标记「待重启更新」、屏蔽 Action/WebUI
        abort "$MSG_INSTALL_CANCELLED"
    fi

    # 模式已选定：再问是否保留已有配置（完整安装与热更新共用这一问）
    ask_keep_config
    
# [hot-update-flow]
    if [ $choice_result -eq 0 ]; then
        ui_print "$MSG_SELECTED_UP"
        ui_print "$MSG_FULL_INSTALL"
    else
        ui_print "$MSG_SELECTED_DOWN"
        ui_print "$MSG_HOT_UPDATE_START"
        
        MODDIR="/data/adb/modules/chiri"
        # 必须 export：安装器环境可能已把 MODDIR 导出为 staging 目录（modules_update），service
        # sh 的 [ -z "$MODDIR" ] 会继承错位路径——看门狗从 staging 拉起 daemon、日志/pidfile/锁全写 staging，KSU 清理后调度静默死亡；
        # export 后子进程强制拿到 live 目录
        export MODDIR
        
        # 1. 停止守护进程和主进程
        # 必须先杀旧看门狗：否则它每 3s 把旧二进制 daemon 拉回，复活落在复制窗口会让 cp 覆盖运行中 chiri 报 ETXTBSY 且被 2>/dev/null 吞掉，
        # 模块残留旧版本、须重启（KSU 应用 modules_update）才被覆盖固化
        ui_print "$MSG_STOPPING_DAEMON"
        PID_FILE="$MODDIR/logs/watchdog.pid"
        if [ -f "$PID_FILE" ]; then
            pid=$(cat "$PID_FILE" 2>/dev/null)
            case "$pid" in
                ''|0|*[!0-9]*) ;;
                *) kill "$pid" 2>/dev/null ;;
            esac
            rm -f "$PID_FILE"
        fi
        if [ -x "/system/bin/killall" ]; then
            /system/bin/killall -9 chiri 2>/dev/null
        elif [ -n "$BUSYBOX" ]; then
            $BUSYBOX killall -9 chiri 2>/dev/null
        fi
        sleep 1

        ui_print "$MSG_STOPPING_MAIN"
        if [ -x "/system/bin/killall" ]; then
            /system/bin/killall -9 chiri 2>/dev/null
        elif [ -n "$BUSYBOX" ]; then
            $BUSYBOX killall -9 chiri 2>/dev/null
        fi
        sleep 1

        # 轮询确认 daemon 真正退出（最多 ~5s）：D 状态进程对 SIGKILL 也要等 IO 返回，未死透时 cp 会 ETXTBSY
        CHIRI_ALIVE=true
        n=0
        while [ $n -lt 5 ]; do
            if [ -x "/system/bin/pidof" ]; then
                DAEMON_PID=$(/system/bin/pidof chiri 2>/dev/null)
            elif [ -n "$BUSYBOX" ]; then
                DAEMON_PID=$($BUSYBOX pgrep -x chiri 2>/dev/null)
            else
                DAEMON_PID=""
            fi
            if [ -z "$DAEMON_PID" ]; then
                CHIRI_ALIVE=false
                break
            fi
            sleep 1
            n=$((n + 1))
        done

        # 2. 复制模块文件到目标目录
        ui_print "$MSG_COPYING_FILES"
        # 备份用户可改配置：config/meta.yaml（抬头字段）与模块根的 rules.yaml；feature.yaml 等仅存于二进制，无需备份
        if [ -f "$MODDIR/config/meta.yaml" ]; then
            cp "$MODDIR/config/meta.yaml" "$MODDIR/config/meta.yaml.bak"
        fi
        if [ -f "$MODDIR/rules.yaml" ]; then
            cp "$MODDIR/rules.yaml" "$MODDIR/rules.yaml.bak"
        fi

        cp -r "$MODPATH"/* "$MODDIR/" 2>/dev/null

        # 二进制单独复制并校验：daemon 未完全退出或内核仍持文本页时 cp 会 ETXTBSY 失败，静默失败即残留旧版；失败时恢复配置备份、保留旧版文件并提示用户手动执行 Action 启动注意：
        # 热更新路径必须以 abort 报错结束（成功与失败皆然）——正常结束会保留 modules_update 暂存，模块被归为「待重启更新」、Action/WebUI 禁用直到重启
        UPDATE_OK=true
        if [ "$CHIRI_ALIVE" = "true" ] || ! cp "$MODPATH/core/bin/chiri" "$MODDIR/core/bin/chiri" 2>/dev/null; then
            UPDATE_OK=false
        fi
        # 关键文件存在性抽查：cp -r 把错误吞进 /dev/null，ENOSPC/IO 错误会留半新半旧模块且无感知；service.sh/module
        # prop/allowHotUpdate/rules.yaml 任一缺失即判失败，走与二进制相同的回滚分支
        for key_file in service.sh module.prop allowHotUpdate rules.yaml; do
           [ -f "$MODDIR/$key_file" ] || UPDATE_OK=false
        done
        if [ "$UPDATE_OK" = "false" ]; then
            if [ -f "$MODDIR/config/meta.yaml.bak" ]; then
                mv "$MODDIR/config/meta.yaml.bak" "$MODDIR/config/meta.yaml"
            fi
            if [ -f "$MODDIR/rules.yaml.bak" ]; then
                mv "$MODDIR/rules.yaml.bak" "$MODDIR/rules.yaml"
            fi
            chmod 755 "$MODDIR/core/bin/chiri" 2>/dev/null
            ui_print "ERROR: hot update copy failed! Old version kept."
            ui_print "错误：热更新文件复制失败！旧版本文件已保留。"
            ui_print "请手动执行 Action 启动旧版本调度。"
            ui_print "Run Action manually to start the old scheduler."
        fi

        # 恢复用户配置：只在「保留配置」这条路盖回覆盖（CONFIG_KEPT=false）时不还原——盖回会顶掉新包 meta.yaml、白写本轮电池校准，
        # 且 ChiRi 机型生效配置是 config/<soc>/meta.yaml 不在备份链，仅回退机型（生效配置即根上 config/meta.yaml）会被旧文件顶回、行为不一致；
        # 备份仍留给复制失败回滚
        if [ "$CONFIG_KEPT" != "false" ]; then
            if [ -f "$MODDIR/config/meta.yaml.bak" ]; then
                mv "$MODDIR/config/meta.yaml.bak" "$MODDIR/config/meta.yaml"
            fi
            if [ -f "$MODDIR/rules.yaml.bak" ]; then
                mv "$MODDIR/rules.yaml.bak" "$MODDIR/rules.yaml"
            fi
        else
            rm -f "$MODDIR/config/meta.yaml.bak" "$MODDIR/rules.yaml.bak"
        fi

        # 设置权限（二进制在 core/bin/chiri，模块根下并无 chiri 文件）
        chmod 755 "$MODDIR/service.sh" 2>/dev/null
        chmod 755 "$MODDIR/action.sh" 2>/dev/null
        chmod 755 "$MODDIR/core/bin/chiri" 2>/dev/null
        # 外部打包脚本（对外暴露的稳定接口，daemon 与 WebUI 共用）
        chmod 755 "$MODDIR/scripts/pack.sh" 2>/dev/null
        
        # 3.【已取消】重启调度服务：安装器环境里 setsid/nohup 拉起的 service.sh 生命周期不可控（管理器退出后可能被收割）、失败也无法可靠提示，
        # 统一改为要求用户手动执行 Action 启动（见下方文案）
        ui_print " "
        ui_print "$MSG_HOT_UPDATE_ABORT"

        # 5. 清理安装暂存 + 走官方失败路径结束安装热更新已把文件直接热替换到 live 目录，若让安装"成功"，
        # 管理器会按 modules_update/<id> 暂存的存在显示「需要重启更新」并屏蔽 Action/WebUI——必须以失败收场
        # 不能用 exit（含 exit 1）：exit 跳过安装器收尾清理，暂存残留磁盘上、之后每次热更新都无法解除，Action/WebUI 一直被屏蔽
        # 正确做法（官方 abort 语义）：
        # 1) 先删两处「待重启」标记：modules_update/chiri（本次暂存内容已热替换进 live，无保留价值，含此前残留旧暂存）
        # 与模块目录内 update 标记文件（上次完整安装遗留的另一判定依据）；清掉后 Action/WebUI 立即恢复、无需重启；
        # 2) abort 打印消息 + 执行收尾清理 + 安装器报失败，后续「完整安装」代码不会执行
        # 顺序约束：必须位于所有 $MODPATH 读取之后；脚本自身由安装器从暂存 source 执行，unlink 不影响已打开 fd
        rm -rf /data/adb/modules_update/chiri
        rm -f /data/adb/modules/chiri/update
        abort "$MSG_HOT_UPDATE_ABORT"
    fi
else
    ui_print "$MSG_HOT_UPDATE_UNAVAILABLE"
    ui_print " "
    # 没有模式选择，但「是否保留现有配置」照问（首次安装时没有旧配置，保留也无害）
    ask_keep_config
fi

# [full-install] 完整安装流程（原逻辑）：模块文件由 Magisk 从暂存目录复制过来；唯一例外是 [battery-detect]——覆盖安装时会按机型修 staged 的 meta.yaml

# 完整安装收尾：电池读数开关与校准倍数已在 [config-keep] 写好（放那儿是为了热更新路径也走得到——热更新以 abort 结束不会回到文件末尾）