#!/system/bin/sh
# notify.sh: [usage] [why] [verdict] [ladder] [result]
# 通知投递的唯一外部出口（daemon 与 WebUI 共用；对外暴露的稳定接口，构建流程不得修改）。
#
# [why] 为什么必须下沉到脚本，以及为什么不看退出码：
#   ① 已核实 AOSP `NotificationShellCmd`：用法 `post [flags] <tag> <text>`，第三位置参数是 **tag**
#      （同 tag 互相覆盖，`NOTIFICATION_ID` 固定 2020）；`-c` 是 `--content-intent`（**不是渠道**），
#      渠道 id 由 AOSP 硬编码为 `shell_cmd`/`Shell command`，**调用方无法指定渠道**——本仓库历史
#      叫法「渠道」实际进的是 tag 位。`-o|--ongoing` **在任何版本都不存在**；`-u|--user` 在
#      Android 13~16-release **也没有**（2025 年才加入），故本脚本一律不传（传了会打 usage 且**仍退出 0**）。
#   ② **退出码 0 毫无意义**：`onCommand` 把 `doNotify` 的返回值丢掉并 `return 0`；只有权限门
#      （非 root/shell）才返回 255。所以「成功」判据只能来自输出，不能来自退出码。
#   ③ **根因（已真机实证，2026-10-17 一加 SM8550 / ColorOS）**：以 root（uid 0）投递时
#      `callingPackage` 被设成 `NotificationManagerService.ROOT_PKG`（字符串 `"root"`，
#      **不是真实安装的包**）→ NMS `fixNotification` 里 `getApplicationInfoAsUser("root", …)`
#      抛 `NameNotFoundException` → 记 `Cannot fix notification` → `return false` **通知被丢弃**。
#      真机两行输出即判决：
#        `su 0    cmd notification post -v …` → `warning: couldn't find notification after enqueueing`（无记录）
#        `su 2000 cmd notification post -v …` → `posted:` + `NotificationRecord(… pkg=com.android.shell
#                                               uid=2000 … key=0|com.android.shell|2020|TAG|2000)`
#      故 uid 0 直投必然丢；`com.android.shell`（uid 2000）是真实包，故能落地。这就是社区那句
#      「If you are root notification will not be displayed」的真机制（不是「root 被禁」——root
#      其实被 `checkShellCommandPermission` 明确放行，且经 AMS `canAccessUnexportedComponents`
#      持有 POST_NOTIFICATIONS）。
#   ④ **换身份也可能静默失败**：MagiskSU 的 optstring 是 `+:himpds:VvZ:Mt:g:G:`，**没有 `l`**；
#      未知选项会 `optind--; goto end` 停止解析并把残余透传给 root shell——于是 `su -lp 2000 -c …`
#      在 Magisk 上**仍是 root**（KernelSU/toybox 则支持 `-l`）。这也是必须看输出验证的原因。
#   ⑤ 命令行构造要引号转义，放 shell 里比放 Rust 里可控；Rust 侧只传参 + 按本脚本退出码留痕。
#
# [usage] notify.sh <title> <tag> <body>
#   tag 即 AOSP 语义的 tag。三参允许为空串，但 tag 为空会让所有通知互相覆盖，脚本直接拒绝（退出码 2）。
#
# [verdict] 每次尝试都判定输出，据此决定「换下一档」还是「收下」——判别串取自 AOSP 源码，非猜测：
#   posted   输出含 `posted:`                      → 真落地，收下（最优）
#   notfound 输出含 `warning: couldn't find notification after enqueueing`
#            → `-v` 已生效但 AOSP 回查不到 ⇒ **这次投递被丢弃**（uid 0 直投、或 su 没真换身份）
#   rejected 输出含 `usage: cmd notification post` / `error: invalid icon` / `permission denied`
#            → 旗标或参数被拒（**注意它仍退出 0**）⇒ 降旗标档重试
#   unknown  以上都没有（老 ROM 不认 `-v`，只打 `posting:`）⇒ 无证据可判，收下
#   `-v` 是关键：它让 AOSP 在 enqueue 后自己 `findNotificationLocked` 回查并打印结论。
#
# [ladder] 外层 su 形式 × 内层旗标档，**按「最可能真生效」排序**：
#   su 形式：`su 2000 /system/bin/cmd …`（argv 直传，**真机实测通过**：不经内层 shell，
#            既免一整层引号转义，又绕开各家 su 的 `-c` 语义差异，故排第一）
#          → `su 2000 -c`（数字 uid + `-c`，Magisk 与 KernelSU 都认）
#          → `su shell -c`（只认用户名、不认数字 uid 的实现）
#          → `su -lp 2000 -c`（toybox/旧 SuperSU；Magisk 上会静默留在 root，靠探针+verdict 兜住）
#          → `su -c`（root，最后手段）／无 su 时 `cmd …` 直投
#   旗标档：`-v -S bigtext` → `-S bigtext` → 最简 `-t`（各 ROM 认的旗标不同）
#   换身份的形式**先用 `id -u` 探针验明身份**（输出含 2000 才通过）：这是与 `-v` 无关的硬证据，
#   在「老 ROM 不认 `-v`、verdict 只能是 unknown」时仍能识破「su 没真换身份」。
#   某形式 verdict=notfound 就**整档跳过**（去掉 `-v` 只会让证据消失、把失败误判成成功）。
#
# [result] stdout 逐行输出，便于 daemon / WebUI 原样留痕与展示：
#   S<n>=<命令行>                     即将尝试的策略
#   S<n>:ok | S<n>:dropped | S<n>:fail:<rc> <首行输出>   该策略结果
#   OK S<n>                          收下（verdict=posted 或 unknown）
#   VERIFY posted                    真落地
#   VERIFY unverified                无证据可判（老 ROM 不认 -v），已尽力
#   VERIFY dropped / LIST ok|miss|unavailable
#   FAIL <原因>                      全部策略都没能落下
#   退出码：0 = 已投递（含 unverified）；1 = 全部失败/被丢弃；2 = 参数非法

TITLE=${1:-}
TAG=${2:-}
BODY=${3:-}

if [ -z "$TAG" ]; then
  echo "FAIL bad-args: tag 为空（AOSP 第三位置参数是 tag，空 tag 会让所有通知互相覆盖）"
  exit 2
fi

# POSIX 单引号转义：整串包单引号，内部每个 ' 换成 '\''（唯一安全形式；循环实现避免 sed 转义歧义）
quote() {
  _s=$1
  _out="'"
  while :; do
    case "$_s" in
      *"'"*)
        _out="$_out${_s%%\'*}'\\''"
        _s=${_s#*\'}
        ;;
      *)
        _out="$_out$_s'"
        break
        ;;
    esac
  done
  printf '%s' "$_out"
}

# verdict：把一次输出的「成没成」判出来（见 [verdict]）
verdict() {
  _o=$1
  case "$_o" in
    *"posted:"*) printf 'posted' ;;
    *"couldn't find notification after enqueueing"*) printf 'notfound' ;;
    *"usage: cmd notification post"*|*"error: invalid icon"*|*"permission denied"*) printf 'rejected' ;;
    *) printf 'unknown' ;;
  esac
}

# 反查 tag 是否真在系统通知列表里；list 不被支持时如实说 unavailable
list_check() {
  _l=$($TIMEOUT /system/bin/cmd notification list 2>/dev/null)
  if [ $? -eq 0 ] && [ -n "$_l" ]; then
    if printf '%s\n' "$_l" | grep -Fq "$TAG"; then
      printf 'ok'
    else
      printf 'miss'
    fi
  else
    printf 'unavailable'
  fi
}

# daemon 在调度线程上同步调用本脚本：任何一步卡住都会拖住调度，故每条命令都加超时，
# 且**整体还受一个总预算约束**——失败级联时 5 种 su 形式 × 3 档旗标最坏能拖几分钟，
# 那既会拖住调度线程，也会超过 WebUI `ksu.exec` 的 20s 上限（超时后界面只会看到「命令执行超时」，
# 把真正的失败原因全丢掉）。单次 5s + 总预算 15s 保证无论如何都远早于调用方超时返回。
TIMEOUT=""
if command -v timeout >/dev/null 2>&1; then
  TIMEOUT="timeout 5"
fi

BUDGET=15
_T0=""
if command -v date >/dev/null 2>&1; then
  _T0=$(date +%s 2>/dev/null)
fi
# 预算用尽则返回 1；拿不到 date 就退化为「不设总预算」（单次超时仍然生效）
budget_left() {
  [ -z "$_T0" ] && return 0
  _now=$(date +%s 2>/dev/null)
  [ -z "$_now" ] && return 0
  [ $((_now - _T0)) -lt $BUDGET ]
}

# su 入口探测：各 Root 方案路径不同；找不到就只剩直投策略
SU=""
for c in /system/bin/su /system/xbin/su /system/bin/ksu /data/adb/ksu/bin/ksu /data/adb/magisk/su; do
  if [ -x "$c" ]; then SU="$c"; break; fi
done
if [ -z "$SU" ]; then
  _cand=$(command -v su 2>/dev/null)
  [ -n "$_cand" ] && SU="$_cand"
fi

N=0
ACCEPTED=""
VERDICT=""
LAST_OUT=""
DROPPED=""

# 跑一条 argv 策略并按 verdict 判定。收下返回 0；drop 返回 2；被拒/执行失败返回 1
attempt() {
  _desc=$1
  shift
  N=$((N + 1))
  echo "S$N=$_desc"
  _out=$($TIMEOUT "$@" 2>&1)
  _rc=$?
  if [ "$_rc" -ne 0 ]; then
    echo "S$N:fail:$_rc $(printf '%s\n' "$_out" | head -n 1)"
    return 1
  fi
  _vd=$(verdict "$_out")
  case "$_vd" in
    posted|unknown)
      echo "S$N:ok"
      ACCEPTED=$N
      VERDICT=$_vd
      LAST_OUT=$_out
      return 0
      ;;
    notfound)
      echo "S$N:dropped"
      DROPPED=$N
      LAST_OUT=$_out
      return 2
      ;;
    *)
      echo "S$N:rejected $(printf '%s\n' "$_out" | head -n 1)"
      return 1
      ;;
  esac
}

# 按 su 形式 + 旗标档执行内部命令
run_su() {
  case "$1" in
    num)  attempt "$2" "$SU" 2000 -c "$3" ;;
    name) attempt "$2" "$SU" shell -c "$3" ;;
    lp)   attempt "$2" "$SU" -lp 2000 -c "$3" ;;
    root) attempt "$2" "$SU" -c "$3" ;;
  esac
}

# 该 su 形式是否**真的**切到了 uid 2000。这一步与 `-v` 无关，故在「老 ROM 不认 -v、
# verdict 只能是 unknown」时仍是硬证据——MagiskSU 没有 `l`，`su -lp 2000` 会静默留在 root，
# 光看 `-v` 缺失的 ROM 就分不出来。探针用 `id -u`（toybox/busybox 都有），输出含 2000 即通过。
# 代价是每条 su 形式多一次调用；本脚本只在启动/FAS 沿/手动测试这类低频路径上跑，可接受
su_identity_ok() {
  case "$1" in
    argv) _pr=$($TIMEOUT "$SU" 2000 /system/bin/id -u 2>/dev/null) ;;
    num)  _pr=$($TIMEOUT "$SU" 2000 -c 'id -u' 2>/dev/null) ;;
    name) _pr=$($TIMEOUT "$SU" shell -c 'id -u' 2>/dev/null) ;;
    lp)   _pr=$($TIMEOUT "$SU" -lp 2000 -c 'id -u' 2>/dev/null) ;;
    *)    return 1 ;;
  esac
  case "$_pr" in
    *2000*) return 0 ;;
  esac
  N=$((N + 1))
  echo "S$N=[probe] su 未真正切到 uid 2000（id -u 输出：$(printf '%s' "$_pr" | head -n 1)），跳过该形式"
  return 1
}

# 外层：su 形式（无 su 时退化为单一直投形式）；内层：旗标档
# argv 排第一：真机实测 `su 2000 cmd …` 可用，且不经内层 shell，免掉一整层引号转义
if [ -n "$SU" ]; then
  FORMS="argv num name lp root"
else
  FORMS="direct"
fi

BUDGET_HIT=""
for _form in $FORMS; do
  [ -n "$ACCEPTED" ] && break
  if ! budget_left; then BUDGET_HIT=1; break; fi
  # 换身份的形式先验身份：探针不通过就整个跳过，省掉 3 档旗标的无谓尝试
  case "$_form" in
    argv|num|name|lp) su_identity_ok "$_form" || continue ;;
  esac
  for _fv in 0 1 2; do
    if ! budget_left; then BUDGET_HIT=1; break 2; fi
    case "$_fv" in
      0) FLAGS="-v -S bigtext"; VLABEL="verbose+bigtext" ;;
      1) FLAGS="-S bigtext";    VLABEL="bigtext" ;;
      *) FLAGS="";              VLABEL="minimal" ;;
    esac
    # 内层命令：FLAGS 是自有定值，需要分词故不加引号；标题/正文/tag 一律 quote
    if [ -n "$FLAGS" ]; then
      INNER="cmd notification post $FLAGS -t $(quote "$TITLE") $(quote "$TAG") $(quote "$BODY")"
    else
      INNER="cmd notification post -t $(quote "$TITLE") $(quote "$TAG") $(quote "$BODY")"
    fi

    case "$_form" in
      num)    run_su num  "[$VLABEL] su 2000 -c \"$INNER\"" "$INNER" ;;
      name)   run_su name "[$VLABEL] su shell -c \"$INNER\"" "$INNER" ;;
      lp)     run_su lp   "[$VLABEL] su -lp 2000 -c \"$INNER\"" "$INNER" ;;
      root)   run_su root "[$VLABEL] su -c \"$INNER\"（root）" "$INNER" ;;
      argv)   attempt "[$VLABEL] su 2000 /system/bin/cmd notification post $FLAGS -t <title> <tag> <body>" \
                "$SU" 2000 /system/bin/cmd notification post $FLAGS -t "$TITLE" "$TAG" "$BODY" ;;
      direct) attempt "[$VLABEL] cmd notification post $FLAGS -t <title> <tag> <body>（无 su，直投）" \
                /system/bin/cmd notification post $FLAGS -t "$TITLE" "$TAG" "$BODY" ;;
    esac
    _r=$?
    [ "$_r" -eq 0 ] && break
    # dropped：该 su 形式没有真换身份（如 Magisk 上的 -lp）。去掉 -v 只会让证据消失、
    # 把失败误判成 unknown 而收下，故整档跳过，换下一个 su 形式
    [ "$_r" -eq 2 ] && break
  done
done

if [ -z "$ACCEPTED" ]; then
  if [ -n "$BUDGET_HIT" ]; then
    echo "FAIL 总预算 ${BUDGET}s 用尽，未找到可用策略（su=${SU:-未找到}）——已尝试的组合见上方 S 行"
    exit 1
  fi
  # 所有策略都被丢弃时，最后反查一次：`-v` 的回查有 500ms×3 轮询窗口，
  # 万一只是登记尚未可见，list 命中就说明其实落下了
  _lc=$(list_check)
  if [ "$_lc" = "ok" ]; then
    echo "OK list"
    echo "LIST ok $TAG"
    exit 0
  fi
  if [ -n "$DROPPED" ]; then
    echo "VERIFY dropped"
    echo "FAIL 投递被丢弃：cmd 接受了但 AOSP 回查不到（uid 0 直投或 su 未真换身份）；list=$_lc"
  else
    echo "FAIL 所有策略均失败（su=${SU:-未找到}）"
  fi
  exit 1
fi

# 收下后报告验证结论：posted 最可信；unknown 说明老 ROM 不认 -v，再用 list 补一次
case "$VERDICT" in
  posted) echo "VERIFY posted" ;;
  *)
    _lc=$(list_check)
    case "$_lc" in
      ok)   echo "LIST ok $TAG" ;;
      miss) echo "VERIFY unverified（老 ROM 不认 -v）；list=miss $TAG" ;;
      *)    echo "VERIFY unverified（老 ROM 不认 -v）" ;;
    esac
    ;;
esac

exit 0
