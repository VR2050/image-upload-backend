Windows 打包指南

目标
- 将后端 Rust 服务构建为 Windows 可执行（.exe），并把前端静态文件与必要目录打包为一个 zip 文件，便于在 Windows 机器上直接解压运行。

先决条件
- Linux 构建（交叉编译）:
  - rustup, cargo
  - 对于 `x86_64-pc-windows-gnu` target: 安装 mingw-w64（apt/yum/macOS brew 等）以提供链接器
  - zip

- Windows 本地构建:
  - Rust toolchain（MSVC 推荐使用 Visual Studio 工具集，GNU 可使用 mingw-w64）
  - PowerShell (自带)

脚本
- Linux: `scripts/package_windows.sh`
- Windows: `scripts/package_windows.ps1`

示例：Linux 交叉编译并打包

1. 安装 target（示例使用 gnu）

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt-get install mingw-w64 zip
```

2. 运行脚本

```bash
./scripts/package_windows.sh x86_64-pc-windows-gnu
```

示例：Windows 本地构建并打包（PowerShell）

```powershell
# 在项目根目录
.
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope Process
.
\scripts\package_windows.ps1 -Target x86_64-pc-windows-msvc -Configuration Release
```

注意事项
- MSVC 构建需要在 Windows 上安装 Visual Studio 的 C++ 工具链。
- 打包的可执行在不同 Windows 系统上可能需要额外运行时（如 Visual C++ Redistributable）。
- 若你的后端使用了本地依赖或特定平台库，交叉编译可能更复杂。

后续建议
- 增加一个简单的 Windows 服务安装脚本（例如 NSSM 或 PowerShell service 注册），方便在 Windows 上作为系统服务运行。
- 为发布构建附加签名步骤（代码签名）。
