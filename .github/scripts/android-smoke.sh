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

# 登录方式切到"私钥"看一眼导入区域，再切回"密码"。
adb shell input tap $((w * 3 / 4)) 940
sleep 1
shot m02b-key-mode
adb shell input tap $((w / 4)) 940
sleep 1

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

# ---- 私钥登录、打字、断线重连 ----
# 宿主机上起 sshd，给当前用户授权一把临时私钥；应用用这把私钥真正登录进来。
# 私钥和主机列表直接写进应用的私有目录（调试包允许 run-as），绕开文件选择器。
sudo systemctl start ssh || sudo service ssh start
rm -f /tmp/nexsh_ci_key /tmp/nexsh_ci_key.pub
ssh-keygen -q -t ed25519 -N "" -f /tmp/nexsh_ci_key
mkdir -p ~/.ssh
cat /tmp/nexsh_ci_key.pub >> ~/.ssh/authorized_keys
chmod go-w ~ && chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys

adb shell am force-stop dev.nexsh.mobile
adb shell "run-as dev.nexsh.mobile sh -c 'cat > files/ci_key'" < /tmp/nexsh_ci_key
cat > /tmp/nexsh_hosts.toml <<EOF
[[host]]
name = "ci-key"
hostname = "10.0.2.2"
port = 22
user = "$(id -un)"
identity_file = "/data/user/0/dev.nexsh.mobile/files/ci_key"
auth = "key"
EOF
adb shell "run-as dev.nexsh.mobile sh -c 'cat > files/hosts.toml'" < /tmp/nexsh_hosts.toml
adb shell run-as dev.nexsh.mobile ls -l files

adb shell am start -W -n dev.nexsh.mobile/.MainActivity
sleep 5
adb shell input tap 400 490
sleep 8
shot m07-key-login

# 点终端取得焦点，敲一条命令。
adb shell input tap 400 900
sleep 2
adb shell input text "echo%snexsh-typing-ok"
adb shell input keyevent 66
sleep 3
shot m08-typed

# 停掉 sshd 并踢掉已有的会话：应用应当发现断线并开始重连。
# 会话进程在新版 OpenSSH 里叫 sshd-session，旧版叫 sshd，两种都匹配；进程列表留作排查。
{ echo "== before"; ps -eo pid,args | grep -i "[s]shd"; } > shots/sshd.txt
sudo systemctl stop ssh.socket ssh.service
sudo pkill -f "sshd[a-z-]*: $(id -un)"
sleep 4
{ echo "== after"; ps -eo pid,args | grep -i "[s]shd"; } >> shots/sshd.txt
shot m09-reconnecting

# sshd 回来之后，下一次重连应当成功，回到"已连接"。
sudo systemctl start ssh.service || sudo systemctl start ssh.socket
sleep 25
shot m10-reconnected

adb shell pidof dev.nexsh.mobile || echo "应用已经不在运行"
adb logcat -d -t 400 "*:W" > shots/logcat.txt
exit 0
