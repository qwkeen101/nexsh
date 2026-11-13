#!/usr/bin/env bash
# 在模拟器里装上刚打出的 APK，走一遍"添加主机 → 连接"，沿途截图。
# 开发机不运行界面，这些截图和日志是检查手机端实际效果的途径。
set -x
apk=mobile/android/app/build/outputs/apk/debug/app-debug.apk
mkdir -p shots
shot() { adb exec-out screencap -p > "shots/$1.png"; }

adb install -r "$apk"
adb logcat -c
adb shell am start -W -n dev.nexsh.mobile/.MainActivity
sleep 6
shot m01-hosts

size=$(adb shell wm size | grep -o "[0-9]*x[0-9]*" | tail -1)
w=${size%x*}; h=${size#*x}
echo "screen: $w x $h"

# 右下角的加号。
adb shell input tap $((w - 126)) $((h - 220))
sleep 2
shot m02-add

# 地址和用户名：10.0.2.2 是模拟器眼里的宿主机，22 端口没人监听，连接会立刻被拒绝。
adb shell input tap 300 460
sleep 1
adb shell input text "10.0.2.2"
adb shell input tap 300 705
sleep 1
adb shell input text "root"
sleep 1
shot m03-filled
adb shell input tap $((w - 90)) 205
sleep 2
shot m04-saved

# 列表里的第一台主机。
adb shell input tap 400 490
sleep 5
shot m05-terminal

# 按键栏：点一下 Ctrl，再点终端唤出键盘。
adb shell input tap 400 1200
sleep 3
shot m06-keyboard

adb shell pidof dev.nexsh.mobile || echo "应用已经不在运行"
adb logcat -d -t 400 "*:W" > shots/logcat.txt
exit 0
