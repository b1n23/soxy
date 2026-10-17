# soxy 前端在 Windows + VMware Horizon（PCoIP / Blast Extreme）下的部署

> 适用范围：**Windows 客户端 + VMware Horizon Client + PCoIP 或 Blast Extreme 显示协议**。
> 如果池配的是 *Microsoft RDP* 显示协议，请改用 README 里的 `soxyreg svc register`
> （mstsc AddIn），本文不适用。

本文覆盖三件事：**自动配置**（`soxyreg horizon`）、**手动配置**、**恢复/回滚**。

---

## 目录

1. [为什么不能照搬 README 的做法](#1-为什么不能照搬-readme-的做法)
2. [自动配置](#2-自动配置)
3. [手动配置](#3-手动配置)
4. [验证](#4-验证)
5. [恢复 / 回滚](#5-恢复--回滚)
6. [故障排查](#6-故障排查)
7. [附录：涉及的注册表结构](#7-附录涉及的注册表结构)

---

## 1. 为什么不能照搬 README 的做法

soxy 的 Windows 前端在 README 里是注册成 mstsc/RDS AddIn：

```
HKCU\Software\Microsoft\Terminal Server Client\Default\AddIns\soxy
    Name = <soxy.dll 路径>
```

这个机制由 `mstscax.dll` 在建立 **RDP** 连接时枚举加载。而 Horizon 使用
**PCoIP / Blast Extreme** 时：

- 不创建 `rdp-tcp#N` 会话，`query session` 只显示 `console` + `rdp-tcp 侦听`；
- **没有任何进程加载 `mstscax.dll`**（`tasklist /m mstscax.dll` 为空）；
- 因此上面的注册表根本不会被读取，`soxyreg svc register` **静默无效**——不报错、
  不打日志，只是什么都没发生。

Horizon 下真正的机制是客户端自己的虚拟通道 DLL 列表：

```
HKLM\SOFTWARE\VMware, Inc.\VMware VDM\Client
    VirtualChannelDLLs    REG_SZ    <DLL 路径>[,{GUID}],<DLL 路径>,...
```

`REG_SZ`，逗号分隔，每条是 `DLL 路径` 或 `DLL 路径,{GUID}`（GUID 可选，
VMware 自带的 ThinPrint 条目就带一个）。VMware 的 `rdpvcbridge` 本质上是一个
"RDP 虚拟通道桥"：它模拟 mstsc AddIn 宿主，把标准 RDS 插件（导出
`VirtualChannelEntry`）的通道数据搬到 PCoIP/Blast 上传输——soxy 前端正是这类插件。

### 三个必须知道的点

| # | 事项 | 说明 |
|---|---|---|
| 1 | **位数** | 前端位数必须匹配 `vmware-remotemks.exe`，**不是** `vmware-view.exe`。前者只是个 32 位启动壳，真正加载插件的是后者，而它即使在 32 位安装下也是 **64 位**。所以一般要用 `x86_64-pc-windows-gnu` 构建 |
| 2 | **前后端位数无需一致** | 虚拟通道只传字节，`"SOXY"` 这个名字对上就能通。64 位前端配 32 位后端完全正常 |
| 3 | **配置文件** | `%APPDATA%\soxy.toml` 里 Windows 路径必须用**正斜杠**或 **TOML 字面量字符串**（单引号）。`"C:\Users\..."` 里的 `\U` 是 TOML 非法转义，会导致整个文件解析失败，而失败是**静默**的（见 §6）|

---

## 2. 自动配置

用 release 包里的 `soxyreg.exe`（**管理员权限**的 cmd）：

```cmd
:: 安装
soxyreg.exe horizon register D:\Software\soxy-release-v4.8.4\frontend\x86_64-pc-windows-gnu\soxy.dll

:: 卸载
soxyreg.exe horizon unregister D:\Software\soxy-release-v4.8.4\frontend\x86_64-pc-windows-gnu\soxy.dll
```

`soxyreg horizon` 做的事：

- 同时写 **两个注册表视图**（`KEY_WOW64_64KEY` 与 `KEY_WOW64_32KEY`），因此结果
  不依赖于 `soxyreg` 自身的位数；
- `register` 幂等：条目已在列表里就不会重复追加；
- `unregister` 精确移除我们的条目（含跟随的 GUID）；删空后连值一起删掉，
  回到"从未装过插件"的状态；
- 未提权时给出明确提示（该值在 `HKLM` 下，普通用户写不了）。

> 注意：`soxyreg` 的位数可以随便选，它用 `KEY_WOW64_*` 标志显式指定视图。
> 但 `soxy.dll` 的位数必须是 64 位（见 §1 第 1 条）。

---

## 3. 手动配置

如果不想用 `soxyreg`，或者需要在脚本/镜像里做，按下面来。

### 3.1 确认 DLL 与位数

```cmd
dir D:\Software\soxy-release-v4.8.4\frontend\x86_64-pc-windows-gnu\soxy.dll
```

`x86_64-pc-windows-gnu` 构建的 `soxy.dll` **静态链接**了 `libgcc`/`libwinpthread`
（Makefile 里 win7 后端有 `+crt-static`，前端同样自带），不需要额外拷贝运行时 DLL。
如果是别的构建，先确认依赖齐全：

```cmd
where libgcc_s_seh-1.dll
where libwinpthread-1.dll
```

### 3.2 追加到 VirtualChannelDLLs

**先读当前值**（两个视图都要读，VMware 自己可能已经注册了 ThinPrint 等）：

```cmd
reg query "HKLM\SOFTWARE\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs
reg query "HKLM\SOFTWARE\WOW6432Node\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs
```

典型现值：

```
C:\Program Files\Common Files\VMware\ThinPrint\TPClnRDP.dll,{DEDC9BA7-C55C-4a3b-965E-B08EAC50D504},
C:\Program Files (x86)\VMware\VMware Horizon View Client\hznclientctrl-vchan-plugin.dll
```

**追加我们的条目**（在末尾加 `,<DLL 路径>`；路径里**不能有逗号**）：

```cmd
reg add "HKLM\SOFTWARE\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs /t REG_SZ /f ^
  /d "C:\Program Files\Common Files\VMware\ThinPrint\TPClnRDP.dll,{DEDC9BA7-C55C-4a3b-965E-B08EAC50D504},C:\Program Files (x86)\VMware\VMware Horizon View Client\hznclientctrl-vchan-plugin.dll,D:\Software\soxy-release-v4.8.4\frontend\x86_64-pc-windows-gnu\soxy.dll"
```

```cmd
reg add "HKLM\SOFTWARE\WOW6432Node\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs /t REG_SZ /f ^
  /d "C:\Program Files (x86)\Common Files\VMware\ThinPrint\TPClnRDP.dll,{DEDC9BA7-C55C-4a3b-965E-B08EAC50D504},C:\Program Files (x86)\VMware\VMware Horizon View Client\hznclientctrl-vchan-plugin.dll,D:\Software\soxy-release-v4.8.4\frontend\x86_64-pc-windows-gnu\soxy.dll"
```

> **视图与 cmd 位数的关系**（这是最容易踩的坑）：
> - 64 位 cmd 里 `HKLM\SOFTWARE\...` = 64 位视图，`HKLM\SOFTWARE\WOW6432Node\...` = 32 位视图；
> - 32 位 cmd 里 `HKLM\SOFTWARE\...` 会被**重定向**到 `WOW6432Node`，也就是只能写到 32 位视图；
> - 要在 32 位进程里写 64 位视图，启动 `C:\Windows\Sysnative\cmd.exe`（64 位 cmd）。
>
> `soxyreg horizon` 用 `KEY_WOW64_*` 标志，没有这个问题。

### 3.3 准备配置文件

`%APPDATA%\soxy.toml`（即 `C:\Users\<你>\AppData\Roaming\soxy.toml`）：

```toml
ip = "127.0.0.1"

[log]
level = "TRACE"
file = 'C:\Users\me\soxy-frontend.log'
```

`file` 用**单引号**（TOML 字面量字符串，不做转义）；等价写法是正斜杠
`"C:/Users/me/soxy-frontend.log"`。

Horizon Client 没有控制台可挂，**一定要设 `log.file`**，否则排查时什么都看不到。

### 3.4 重启客户端

**完全退出 Horizon Client**（托盘图标一起退），再启动、连接。

重连会话不够——这个列表是客户端启动时读的。

---

## 4. 验证

### 4.1 前端是否加载（看日志）

`type C:\Users\me\soxy-frontend.log`，出现这几行就是成功了：

```
[INFO ] frontend version 4.8.4
[DEBUG] CALLED VirtualChannelEntry
[INFO ] binding clipboard clients on 127.0.0.1:3032
[INFO ] binding command clients on 127.0.0.1:3031
[INFO ] binding ftp clients on 127.0.0.1:2021
[INFO ] binding input clients on 127.0.0.1:1081
[INFO ] binding socks5 clients on 127.0.0.1:1080
[INFO ] binding stage0 clients on 127.0.0.1:1082
[INFO ] (control channel) changing to LOADED state
[INFO ] (control channel) SVC(RDP) name is "SOXY"
[INFO ] (control channel) changing to OPENED state      ← 通道已建立
```

### 4.2 端口是否监听

```cmd
netstat -ano | findstr ":1080 :3031 :3032 :2021 :1081 :1082"
```

| 端口 | 服务 |
|---|---|
| 1080 | socks5 |
| 2021 | ftp |
| 3031 | command（远端 shell）|
| 3032 | clipboard |
| 1081 | input |
| 1082 | stage0 |

### 4.3 端到端功能

```cmd
curl -v -x socks5://127.0.0.1:1080 http://example.com
nc 127.0.0.1 3031
ftp 127.0.0.1 2021
```

`curl` 能拿到 HTTP 响应 = 整条链路（前端 → Horizon rdpvcbridge → Blast 虚拟通道 →
远端 `vdp_rdpvcbridge` → 后端 → 目标网络）全部打通。

### 4.4 后端（远端机器）

后端位数匹配**远端**机器（32 位 Win7 用 `i686-win7-windows-gnu`），在 Horizon 会话里
从 cmd 运行：

```cmd
D:\debug\backend\i686-win7-windows-gnu\soxy.exe
```

看到 `virtual channel SOXY opened SVC(high) "SOXY"` 即通。

---

## 5. 恢复 / 回滚

### 5.1 自动卸载前端

```cmd
soxyreg.exe horizon unregister D:\Software\soxy-release-v4.8.4\frontend\x86_64-pc-windows-gnu\soxy.dll
```

输出 `64-bit view: removed` / `32-bit view: removed` 即成功。

### 5.2 手动卸载前端

**方式 A：只删我们的条目**（保留 ThinPrint 等 VMware 自己的条目，推荐）

读出当前值，把我们的 `,D:\...\soxy.dll` 那段去掉，再写回去：

```cmd
reg query "HKLM\SOFTWARE\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs
reg add "HKLM\SOFTWARE\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs /t REG_SZ /f ^
  /d "<去掉我们条目后的值>"
reg add "HKLM\SOFTWARE\WOW6432Node\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs /t REG_SZ /f ^
  /d "<去掉我们条目后的值>"
```

**方式 B：整个值删掉**（只有确认没有其他插件时才能这么干）

```cmd
reg delete "HKLM\SOFTWARE\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs /f
reg delete "HKLM\SOFTWARE\WOW6432Node\VMware, Inc.\VMware VDM\Client" /v VirtualChannelDLLs /f
```

> 空值和"没有这个值"对 Horizon Client 是等价的（都表示从未装过插件），
> 所以删空是安全的状态。

### 5.3 清理配置文件与日志

```cmd
del "%APPDATA%\soxy.toml"
del C:\Users\me\soxy-frontend.log
```

### 5.4 如果之前还注册过 mstsc AddIn（`soxyreg svc register`）

一并清掉，避免以后切到 Microsoft RDP 显示协议时它又冒出来：

```cmd
soxyreg.exe svc unregister
```

### 5.5 重启客户端

同 §3.4，完全退出（含托盘）再启动。

### 5.6 后端侧

后端没有安装任何东西，只是个可执行文件。停掉即可：

- 在跑 `soxy.exe` 的 cmd 里按 `Ctrl+C`；或
- `taskkill /im soxy.exe /f`

删除 `D:\debug\backend\i686-win7-windows-gnu\` 目录即彻底清理。

---

## 6. 故障排查

soxy 在 Horizon 下的失败几乎全是**静默**的，下面是本次实际遇到的全部情况。

| 现象 | 原因 | 处理 |
|---|---|---|
| 日志文件根本不出现，也没有控制台窗口 | `%APPDATA%\soxy.toml` 解析失败。`"C:\Users\..."` 里的 `\U` 是 TOML 非法转义，`bootstrap()` 在读配置阶段就返回 `Err`，而 `AllocConsole()` 在**那之后**才调用 | 改用单引号或正斜杠，见 §3.3 |
| 日志出现 `CALLED VirtualChannelEntry` + 六个 `binding ...`，但之后没有 `OPENED` | 前端加载成功，通道没建立——通常是后端没跑或后端崩 | 去远端看后端日志 |
| 日志停在 `trying to open SVC(high)` 之后，进程消失，无对话框 | 后端调 `VDP_VirtualChannelOpen` 时调用约定不匹配（`__cdecl` 被当成 `__stdcall` 调），栈失衡在函数返回时爆掉 | 用 4.8.5+ 的后端 |
| `found libraries: WTS`（没有 Horizon）| 远端 Horizon Agent 没装全，`vdp_rdpvcbridge.dll` 不在 | `where vdp_rdpvcbridge.dll` 确认 |
| `failed to open SVC: open failed (函数不正确 / os error 1)` | 用了 `SOXY_SVC_LOW=1` 或走了 WTS 路径。Blast/PCoIP 下没有 RDS 会话，`WTSVirtualChannelOpen` 必然返回 `ERROR_INVALID_FUNCTION` | 去掉 `SOXY_SVC_LOW`，让后端走 Horizon 的 VDP API |
| `tasklist /m mstscax.dll` 为空 | 显示协议是 Blast/PCoIP，RDP 栈没加载 | 正常，说明必须用本文的方法而不是 mstsc AddIn |
| 前端日志有 `open virtual channel` 但 `soxy-frontend.log` 里没有任何内容 | `log.file` 没配，而 Horizon Client 没有控制台可挂 | 见 §3.3 |

### 提升 VMware 侧日志级别

Horizon Agent 的 rdpvcbridge 自己的日志：

```cmd
reg add "HKLM\SOFTWARE\VMware, Inc.\VMware VDM\RdpVcBridge" /v logLevel /t REG_SZ /d INFO /f
```

日志在 `%TEMP%\vmware-<用户>\vmware-rdpvcbridge-<通道名>-<PID>.log`。
`soxy.exe` 每次运行都会重新加载 DLL，不用重启服务。

> 该值**不要**设成 `TRACE`：VMware 的级别解析不接受这个字面值，会导致整个日志
> 被关掉（连 banner 都不写），看起来像"日志为空"。

---

## 7. 附录：涉及的注册表结构

### 7.1 前端（客户端，Win11）

| 键/值 | 视图 | 用途 |
|---|---|---|
| `HKLM\SOFTWARE\VMware, Inc.\VMware VDM\Client` → `VirtualChannelDLLs`（`REG_SZ`）| 64 位 | Horizon PCoIP/Blast 的插件列表 |
| `HKLM\SOFTWARE\WOW6432Node\VMware, Inc.\VMware VDM\Client` → 同上 | 32 位 | 同上 |
| `HKCU\Software\Microsoft\Terminal Server Client\Default\AddIns\soxy` → `Name` | — | 仅 Microsoft RDP 显示协议有效 |
| `HKLM\SOFTWARE\VMware, Inc.\VMware VDM\Client\vvc\Plugins\<名>` → `filename` | 64 位 | VMware 自己的 vvc 插件（`scredir_vchanclient`、`vdpService`、`vdp_rdpvcbridge` 等），**不要**往里加 soxy |

`vvc\Plugins` 是另一套机制，条目要求导出 `VDPService_PluginInit` 等接口，
soxy 前端没有这些入口点，放进去不会生效。

### 7.2 配置文件

| 路径 | 说明 |
|---|---|
| `%APPDATA%\soxy.toml` | `dirs::config_dir()`；没有就用默认值并自动生成 |
| `log.file` 指定的路径 | 前端日志；Horizon 下唯一的输出途径 |

### 7.3 后端（远端，Win7）

无需任何注册表改动。相关路径：

| 路径 | 说明 |
|---|---|
| `C:\Windows\System32\vdp_rdpvcbridge.dll` | Horizon Agent 侧的 VDP 虚拟通道 API（32 位）|
| `%TEMP%\vmware-<用户>\vmware-rdpvcbridge-soxy-<PID>.log` | 后端打开 "SOXY" 通道时桥自己写的日志 |
| `HKLM\SOFTWARE\VMware, Inc.\VMware VDM\RdpVcBridge\logLevel` | 桥的日志级别 |
