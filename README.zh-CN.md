<p align="right"><a href="./README.md">EN</a> | <strong>简</strong></p>

<div align="center">
    <img src="packaging/icon/omenctl-gui-128.png" alt="omenctl 图标" width="96">
    <h1>omenctl</h1>
</div>

<p align="center">
    <em>在 Linux 上控制 HP OMEN 16（主板 8D3F）的性能模式和风扇。</em>
</p>

<p align="center">
    <a href="https://github.com/ZerunW/omen-ctl/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/ZerunW/omen-ctl/ci.yml?branch=main&style=flat-square&label=CI" alt="CI 状态" /></a>
    <a href="https://github.com/ZerunW/omen-ctl/releases/latest"><img src="https://img.shields.io/github/v/release/ZerunW/omen-ctl?style=flat-square&color=22c55e" alt="最新版本" /></a>
    <img src="https://img.shields.io/badge/HP%20OMEN%2016-8D3F-C8102E?style=flat-square" alt="HP OMEN 16，主板 8D3F" />
    <img src="https://img.shields.io/badge/Ubuntu-24.04-E95420?style=flat-square&logo=ubuntu&logoColor=white" alt="Ubuntu 24.04" />
    <img src="https://img.shields.io/badge/kernel-7.x-64748b?style=flat-square&logo=linux&logoColor=white" alt="Linux 7.x" />
    <img src="https://img.shields.io/badge/Rust-1.97%2B-B7410E?style=flat-square&logo=rust&logoColor=white" alt="Rust 1.97 或更新" />
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--2.0--or--later-A855F7?style=flat-square" alt="许可证：GPL-2.0-or-later" /></a>
</p>

<div align="center">
    <img src="docs/screenshots/dark.png" alt="OMEN Control 深色主题" width="820">
</div>

## omenctl 是什么？

omenctl 由四部分组成：打过补丁的 `hp-wmi` 内核驱动、以 root 运行的后台服务（`omenctld`）、
命令行工具（`omenctl`）和图形界面（OMEN Control），适用于 HP OMEN 16-am0xxx。它发送和 Windows 上
OMEN Gaming Hub 相同的固件命令，解开 CPU 和显卡的功率限制；由于固件在这种状态下不再自己管风扇，
它还负责用软件驱动风扇。所有东西打包成一个 `.deb`，`apt remove` 可以全部移除。

> [!WARNING]
> omenctl 以 root 身份运行，加载修改过的内核驱动，并改变嵌入式控制器（EC，负责风扇和功率的单片机）
> 控制风扇和功率上限的方式。它只在一台机器上测试过（主板 `8D3F`，BIOS F.14），在其他主板上会拒绝运行。
> 安装前请先阅读[安全设计](#安全设计)。本软件不提供任何担保，使用风险自负。

## 为什么需要 omenctl？

在 Linux 下没有程序给这台笔记本的 EC 发"性能"命令，所以 CPU 一直被限制在 PL1 45 W / PL2 56 W
（平衡模式下是 35/45 W），RTX 5060 被限制在约 80 W。让 EC 进入性能状态不难，难的是让它保持在这个状态：
功率上限只在软件持续手动控制风扇时才有效，而且 EC 在这个状态附近的风扇行为有不少坑（转速目标写 0 表示
*停转*；把风扇交还 BIOS 时风扇可能停两分钟）。omenctl 处理了这些细节，并加上了无人值守运行所需的安全保护。

**状态：测试版。** 作者每天在自己的电脑上使用。驱动、后台服务、命令行工具和图形界面都已在真机上测试；
[真机测试清单](docs/HARDWARE_TEST.md)（英文）还有部分项目没做完。

## 快速开始

在 HP OMEN 16-am0xxx（`cat /sys/class/dmi/id/board_name` 输出 `8D3F`）上，使用 Ubuntu 24.04 和
HWE 7.0 内核，从[最新发布](https://github.com/ZerunW/omen-ctl/releases/latest)下载
`omenctl_<版本>_amd64.deb`，然后：

```bash
sudo apt install ./omenctl_*_amd64.deb
sudo usermod -aG omenctl $USER               # 注销后重新登录生效
sudo reboot                                  # 加载打过补丁的驱动
omenctl status
omenctl mode unleashed
omenctl-gui                                  # 或者在应用菜单里打开 "OMEN Control"
```

## 功能

### 模式

- **Power Saver（省电）、Balanced（平衡）、Unleashed（满血）** — 满血模式把 EC 的 CPU 上限提高到
  PL1 90 W / PL2 130 W / PL4 200 W，显卡上限提高到 115 W。
- **按负载切换 EPP** — 满血模式下，CPU 忙时能耗偏好为 `performance`（单核 4.85 GHz），
  空闲时为 `balance_performance`（约 16 W 而不是 31 W）。
- **显卡优先** — 满血模式下显卡忙时，CPU 持续功率降到 70 W，CPU 和显卡同时满载时显卡上限是 105 W 而不是 85 W。
- **默认模式** — 开机时、插上电源时、用电池时、电量低时各自切换到指定模式。

### 风扇

- **四种风扇模式** — BIOS 自动、每个风扇一条曲线（默认用 BIOS 自带的风扇表）、固定转速、最高转速。
- **平稳的曲线** — 使用 5 秒平均温度，升速后保持 15 秒再降，短暂的负载尖峰不会让风扇忽快忽慢。
- **推荐转速** — 手动转速低于曲线推荐值时允许设置，但会给出提示。

### 监控和界面

- **供电信息** — CPU 和显卡功率及 90 秒曲线、适配器额定功率（读自 BIOS）、电池电量、耗电、剩余时间、
  健康度和循环次数、USB-C 供电。
- **图形界面** — 自动 / 浅色 / 深色主题，自绘窗口和 GNOME 风格按钮，字号跟随系统文字缩放，
  可拖动编辑风扇曲线。
- **命令行** — `omenctl status | mode | fan | defaults`，支持 `status --json`。

### 安装包和安全

- **四层安全保护** — 内核看门狗、带安全脚本的 systemd 看门狗、风扇转速核对、温度保护。
  任何故障都会朝"风扇更快、功率更低"的方向处理。
- **一个 `.deb`** — DKMS 驱动（内核更新后自动重新编译并用 MOK 密钥签名）、后台服务、工具、
  `nvidia-powerd` 设置，卸载时完整回滚。

（程序界面和 `docs/` 下的文档都是英文。）

| 浅色主题，USB-C 供电（90 W） | 默认模式 |
|---|---|
| ![浅色主题](docs/screenshots/light.png) | ![默认模式卡片](docs/screenshots/default-modes.png) |

## 实测结果

HP OMEN 16-am0xxx，Ubuntu 24.04，HWE 内核 7.0，BIOS F.14。

| | 原版 Linux | omenctl 满血模式 |
|---|---|---|
| EC 的 CPU 功率上限 PL1 / PL2 / PL4 | 45 / 56 / 90 W | 90 / 130 / 200 W |
| CPU 单线程 | – | 4.94 GHz |
| CPU 8 线程（每个性能核一个） | – | 4.41 GHz @ 130 W |
| CPU 全部 24 线程，前约 50 秒 | – | 性能核 4.0–4.6 / 能效核 3.3–3.7 GHz，130 W |
| CPU 全部 24 线程持续负载 | 约 2.45 GHz @ 约 45–56 W | 性能核 3.5–3.78 / 能效核 2.8–3.03 GHz，90 W |
| 显卡功率上限 | 80–83 W | 115 W |
| CPU 和显卡同时满载（持续） | – | CPU 70 W + 显卡上限 105 W（显卡优先） |

频率范围取决于负载类型。`tools/stress-test.sh` 可以复现 CPU、显卡和两者同时的测试（普通用户运行，
由 omenctld 负责风扇和保护）；每条 EC 结论是怎么验证的，见
[docs/internals.md](docs/internals.md#4-verified-behaviour)（英文）。

## 兼容性

| | 支持情况 |
|---|---|
| 笔记本 | HP OMEN 16-am0xxx，主板 `8D3F`（Intel Core，RTX 5060）。在其他主板上后台服务和安全脚本什么都不做。 |
| BIOS | F.14（已测试） |
| 发行版 | Ubuntu 24.04 |
| 内核 | 7.x（已测试：HWE 7.0）。DKMS 不会为其他内核编译，它们继续使用原版驱动。 |
| 安全启动 | 关闭，或开启并已注册 Ubuntu 的 DKMS MOK 密钥 |
| 显卡 | 带 `nvidia-powerd` 的 NVIDIA 驱动（显卡功率提升需要它） |

其他 OMEN 和 Victus 主板的 EC 布局不同，不受支持。欢迎同一主板在其他 BIOS 版本上的测试反馈。

## 安装

要求：当前内核的内核头文件（`linux-headers-generic-hwe-24.04`）；安全启动要么关闭，要么已经注册了
给 DKMS 用的 MOK 密钥（安装第三方驱动时 Ubuntu 会引导你注册，可以用 `mokutil --list-enrolled` 查看）。

```bash
sudo apt install ./omenctl_<版本>_amd64.deb
sudo usermod -aG omenctl $USER
sudo reboot
```

安装过程会创建 `omenctl` 组，用 DKMS 编译驱动，把 `omenctld.service` 设为下次开机启动，启用
`nvidia-powerd`，并把示例配置复制到 `/etc/omenctl/config.toml`。重启后检查：

```bash
modinfo -n hp_wmi                            # .../updates/dkms/hp-wmi.ko*
systemctl status omenctld
omenctl status
```

**卸载。** `sudo apt remove omenctl` 移除程序和 DKMS 驱动，把风扇交还 BIOS，并恢复 NVIDIA 相关服务的设置；
重启后加载原版驱动。`sudo apt purge omenctl` 还会删除 `/etc/omenctl`、`/var/lib/omenctl`，
以及 `omenctl` 组（如果这个组是安装包创建的）。

## 使用

```bash
omenctl status                    # 或：omenctl status --json
omenctl mode unleashed            # saver | balanced | unleashed
omenctl fan curve                 # auto | curve | max | 18..52（单位 100 rpm）
omenctl fan 30                    # 固定 3000 rpm
omenctl defaults ac unleashed     # startup | ac | battery | low，然后 keep | saver | balanced | unleashed
omenctl defaults low-at 15        # 低电量阈值，5-50 %
omenctl-gui --theme dark          # system | light | dark，只对这次运行有效
```

满血模式下 `fan auto` 会被拒绝（否则 EC 会撤销提高的功率上限）。出错时退出码为 1。图形界面和每条命令的
详细说明见[使用说明](docs/USER_GUIDE.md)（英文）。

## 配置

`/etc/omenctl/config.toml` 包含默认模式、模式规则、风扇曲线平滑参数、EPP 和显卡优先选项、
每个模式可选的 RAPL 上限，以及各项保护阈值。每个设置都有安全的默认值，未知的设置项会写进日志，
配置文件无效时服务拒绝启动。修改后运行 `sudo systemctl restart omenctld` 生效。完整的设置表见
[使用说明](docs/USER_GUIDE.md#8-configuration)；带注释的示例是 [`packaging/config.toml`](packaging/config.toml)。

## 从源码编译

需要 Rust 1.97 或更新版本（或 Ubuntu 的 `cargo-1.97`）、`dpkg-dev`、`pkg-config`、`libwayland-dev`、
`libxkbcommon-dev`，以及下载 Rust 依赖所需的网络。

```bash
git clone https://github.com/ZerunW/omen-ctl
cd omen-ctl
(cd app && cargo test)            # 单元测试，外加一个在模拟 sysfs 目录上运行后台服务的端到端测试
packaging/build-deb.sh            # 生成 packaging/out/omenctl_<版本>_amd64.deb
```

`CARGO=cargo-1.97 packaging/build-deb.sh` 使用 Ubuntu 的带版本号的 cargo。编译不会改动系统。
CI 在每次推送时运行测试并编译 `.deb`；推送 `v*` 标签时自动发布新版本并附上安装包。

## 工作原理

```text
omenctl-gui（普通用户）   omenctl（普通用户）
          \                 /
      /run/omenctl.sock（omenctl 组可用，每行一条 JSON）
                   |
     omenctld（root，systemd Type=notify，带看门狗）
                   |   只读写标准 sysfs 文件
   platform_profile · hwmon pwm · omenctl_kick · omenctl_concurrent_tdp · omenctl_adapter_w
                   |
     hp-wmi（打过补丁，DKMS）  ->  BIOS WMI  ->  EC
```

以下来自 8D3F 反编译的 BIOS 和 EC 读数：

- WMI 查询 `0x1A` 把模式字节写进 EC 的 `0x95`；在 `0x31` 下，EC 把 PL1/PL2/PL4 提高到 90/130/200 W。
- 查询 `0x10` 会打开一个约 120 秒的窗口，窗口内 EC 按软件给定的转速（`0x2E`）运行风扇。
  性能模式的功率上限只在这个窗口保持期间有效，所以风扇必须由软件驱动。驱动在手动风扇模式下每 90 秒续一次窗口。
- `0x2E` 转速目标写 0 表示**停转**，而不是"自动"。
- 查询 `0x29` 的第 4 个字节设置 NVIDIA Dynamic Boost 使用的 CPU+显卡功率目标；写 75 W
  （OMEN Gaming Hub 用的值）后显卡上限变为 115 W。

在上游 `hp-wmi` 之上的补丁只对 8D3F 生效，增加了：75 W 并发功率目标、内核风扇看门狗（`omenctl_kick`）、
交还 BIOS 时保持当前转速而不是停转的平滑交接，以及只读的适配器额定功率。用户态程序从不直接写 EC。
详细内容、EC 字段表和实测验证见 [docs/internals.md](docs/internals.md)（英文）。

### 仓库结构

| 路径 | 内容 |
|---|---|
| `app/omenctl-core` | 风扇曲线、配置、sysfs 读写、控制循环、通信协议 |
| `app/omenctld` | 后台服务 |
| `app/omenctl-cli` | 命令行工具 `omenctl` |
| `app/omenctl-gui` | 图形界面（egui） |
| `hpwmi-upstream/` | 上游 `hp-wmi.c`、omenctl 补丁，以及 DKMS 编译的打过补丁的源码（[来源说明](hpwmi-upstream/README.md)） |
| `packaging/` | `.deb` 打包脚本、systemd 单元、DKMS 配置、安装/卸载脚本和安全脚本、图标 |
| `tools/` | 只读的 EC 快照脚本、ACPI 提取脚本（`dump-acpi.sh`）和真机测试脚本 |
| `docs/` | 使用说明、内部原理、真机测试清单、截图（英文） |

## 安全设计

omenctl 修改的固件设置会影响散热。设计上假定总有一天会出问题，因此让每种故障都以"风扇更快、功率更低"收场：

- **内核看门狗** — 风扇处于手动模式时，后台服务每秒向驱动报到一次；超过 15 秒没有报到，
  驱动自己把两个风扇设到最高转速。
- **systemd 看门狗** — 卡住的服务会被强制结束并重启；任何异常退出后，安全脚本把风扇设到最高、
  档位切回平衡，等 CPU 低于 60 °C 后才把风扇交还 BIOS。
- **转速核对** — 实际转速连续 10 秒低于目标的 70%：风扇设到最高，退出满血模式。
- **温度保护** — 达到 92 °C 时，任何风扇模式下两个风扇都立即设到最高；95 °C 持续 10 秒则退出满血模式，
  直到温度降下来。读不到 CPU 温度按过热处理。
- **只用有限的命令** — 只发送在 OMEN Gaming Hub 上观察到过的数值；固件自带的"满速"开关在这个型号上
  可能导致风扇卡死，从不使用。

它防不住的情况：与 BIOS F.14 行为不同的固件或 EC、被强行绕过检查的非 8D3F 主板，以及物理散热问题
（出风口堵塞、风扇损坏）。CPU 和显卡自身的过热降频保护仍然有效。报告安全问题请见 [SECURITY.md](SECURITY.md)。

## 常见问题

<details>
<summary><strong><code>omenctl: cannot reach the omenctl service</code></strong></summary>

查看 `systemctl status omenctld` 和 `journalctl -u omenctld -b`。如果服务显示为 "skipped"，说明打过补丁的
驱动没有加载：`modinfo -n hp_wmi` 应该指向 `updates/dkms`；再查看 `dkms status` 和
`journalctl -k -b | grep -i hp_wmi`。

</details>

<details>
<summary><strong>没有权限使用 <code>/run/omenctl.sock</code></strong></summary>

`id` 的输出里应该有 `omenctl`。执行 `usermod -aG omenctl $USER` 后要注销并重新登录。

</details>

<details>
<summary><strong>开启安全启动后驱动无法加载</strong></summary>

查看 `mokutil --sb-state`，确认 DKMS 密钥已经注册；`modinfo hp_wmi | grep signer` 显示模块由哪个密钥签名。

</details>

<details>
<summary><strong>显卡功率上限没有变化</strong></summary>

查看 `systemctl status nvidia-powerd`（或 `omenctl-nvidia-powerd`），然后在显卡负载下运行
`nvidia-smi -q -d POWER`。

</details>

<details>
<summary><strong>风扇一直满速</strong></summary>

`cat /sys/devices/platform/hp-wmi/omenctl_kick`：`tripped=1` 表示内核看门狗触发了，检查后台服务是否在运行。

</details>

<details>
<summary><strong>蓝牙键盘没法把电脑从休眠中唤醒</strong></summary>

那是 BlueZ 的问题，和 OMEN 无关，见配套项目
[ubuntu-ble-wake-fix](https://github.com/ZerunW/ubuntu-ble-wake-fix)。

</details>

更多情况，以及不依赖安装包工具的紧急回滚方法，见[使用说明](docs/USER_GUIDE.md#11-troubleshooting)（英文）。

## 文档

- [使用说明](docs/USER_GUIDE.md) — 安装、模式、风扇、图形界面、命令行、配置、排错、回滚
- [内部原理](docs/internals.md) — WMI 命令、EC 字段、实测验证、驱动补丁
- [真机测试清单](docs/HARDWARE_TEST.md) — 带回滚步骤的逐项测试
- [更新日志](CHANGELOG.md)

（以上文档均为英文。）

## 参与贡献

欢迎报告问题、提交其他 BIOS 版本的测试结果和 Pull Request。编译、测试方法以及哪些改动需要真机证据，
见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 致谢

- Linux 的 [`hp-wmi`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/platform/x86/hp/hp-wmi.c)
  驱动及其作者。omenctl 的驱动是主线提交 `e931e54edb0f` 加一个小补丁（GPL-2.0-or-later），
  见 [hpwmi-upstream/README.md](hpwmi-upstream/README.md)。
- [ohman](https://github.com/P4R1H/ohman) 的 OMEN 固件研究。
- [OmenMon-Reborn](https://github.com/metamountain/OmenMon-Reborn-8BCA) 关于 8D3F 风扇满速卡死的报告（#137）。
- [hp-omen-8603-linux](https://github.com/PickleHik3/hp-omen-8603-linux) 关于相近 OMEN 主板的笔记。
- [omen_max_gpu_unlock](https://github.com/WangSteve123/omen_max_gpu_unlock)，`nvidia-powerd` 的 D-Bus 策略以它为基础。
- 图形界面使用 [egui / eframe](https://github.com/emilk/egui)，显卡读数使用
  [nvml-wrapper](https://github.com/rust-nvml/nvml-wrapper)，界面字体是
  [IBM Plex Sans](https://github.com/IBM/plex)（SIL OFL 1.1）。

## 许可证

[GPL-2.0-or-later](LICENSE) © [Zerun Wang](https://github.com/ZerunW)

驱动源自 Linux 内核的 `hp-wmi.c`。图形界面链接了 Apache-2.0 许可的库（winit、glutin），所以 `omenctl-gui`
这个程序整体按 GPL-3.0-or-later 分发。随附的 IBM Plex Sans 字体使用 SIL Open Font License 1.1
（[`app/omenctl-gui/assets/IBMPlexSans-LICENSE.txt`](app/omenctl-gui/assets/IBMPlexSans-LICENSE.txt)）。
所有 Rust 库的许可证声明安装在 `/usr/share/doc/omenctl/THIRD-PARTY-LICENSES.txt.gz`。HP 固件的 ACPI 表
不在本仓库中（见 `tools/dump-acpi.sh`）。
