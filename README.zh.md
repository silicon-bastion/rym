# Rym

> 一门为龙芯 LoongArch64 设计的系统编程语言，专为驱动 [Rymos](https://github.com/silicon-bastion/rymos) 操作系统而生。

[![CI](https://github.com/silicon-bastion/rym/actions/workflows/ci.yml/badge.svg)](https://github.com/silicon-bastion/rym/actions)
[![License: Mulan PSL v2](https://img.shields.io/badge/license-Mulan%20PSL%20v2-blue.svg)](LICENSE)
[![English](https://img.shields.io/badge/docs-English-blue.svg)](README.md)

---

## 什么是 Rym？

Rym 是一门小而专注的系统编程语言，有三个目标：

1. **绝对清晰** — 每一次所有权转移、内存分配和错误路径都必须显式写出，不存在任何隐式行为。
2. **零隐藏层** — 无垃圾回收、无运行时异常、无隐式 ABI、无包管理器。
3. **龙芯优先** — 从第一行代码起就为龙芯 LoongArch64 处理器设计，不携带任何 x86 历史包袱。

Rym 有意保持小众。它不是要取代 Rust 或 C，它只为把一个操作系统写好而存在。

---

## 语言一览

```rym
// 定义区 — 所有类型和函数在此声明
type 请求 {
    路径: str
    方法: str
    体: []u8
}

fn 解析(原始: read []u8, 分配: read Allocator) -> Result(请求, str) {
    定 路径 = 提取路径(原始).or_return?
    定 方法 = 提取方法(原始).or_else("GET")
    定 体   = 分配.alloc(u8, 原始.长度).or_zero
    return Ok(请求{ 路径=路径, 方法=方法, 体=体 })
}

fn 路由(请求: read 请求, 分配: read Allocator) -> Result(响应, str) {
    定 内容 = 分配.alloc(u8, 256).or_panic("路由必须成功")
    复制(内容, "你好，Rym！")
    return Ok(响应{ 状态=200, 内容=内容 })
}

fn 发送(响应: read 响应, 连接: 连接) -> Result(void, str) {
    定 写入 = syscall(write, 连接.fd, 响应.内容.ptr, 响应.内容.len)
    if 写入 != 响应.内容.长度 { return Err("发送不完整") }
    return Ok(void{})
}

// 算法区 — 只有管道拓扑，不写定义
定 分配器 = SystemAllocator{}
定 连接   = 接受连接()

连接.接收()
    |> 解析(分配器)
    |> 路由(分配器)
    |> 发送(连接)
```

---

## Rym 的独特之处

### 实时节点编辑器（类 Unity）
`rym` 工具链内置可视化编辑器，**在你输入代码的同时实时渲染管道拓扑图和 AST 树形图**。每一个 `|>` 管道连接、每一个所有权模式、每一个函数边界都以活动节点图的形式呈现，而不是静态示意图。编辑器本身用 Rym 编写，完全可定制：可以随意调整面板布局、添加自定义视图、更改整体主题。

### 双环，双性格

| | `base` 环 | `safe` 环 |
|-|-----------|-----------|
| **最接近的类比** | Zig + Odin | Zig + Jam |
| **内存** | 手动管理，显式 SOA 数据布局，零开销抽象 | RAII + 显式分配器 |
| **安全** | 无——开发者完全掌控 | 编译器强制所有权检查 |
| **硬件** | 内联 LoongArch64 汇编，直连系统调用，MMIO | 标准库 IPC 接口 |
| **适用场景** | 编译器扩展、OS 内核、设备驱动 | 应用业务逻辑、OS 服务层 |

> *Jam* 是一门介于 Zig 和 Rust 之间的语言——拥有符合人体工程学的错误处理，但不引入完整的借用检查器。`safe` 环采用同样的设计哲学。

### 中文优先工具链
所有编译器错误信息、编辑器 UI、标准库文档均提供中文版本。中文标识符是一等公民——`fn 解析请求(原始: read []u8)` 是合法的 Rym 代码。两个保留中文关键字 `定`（不可变）和 `设`（可变）对应中文语义，而非英文借词。

### 设计来源

| 特性 | 灵感来源 |
|------|---------|
| 显式分配器参数 | Odin、Zig |
| SOA（结构体数组）数据布局 | Odin |
| 值类型——结构体按值传递，无隐式间接层 | MoonBit |
| 错误处理操作符（`or_return`、`or_panic`、`or_else`、`or_zero`、`or_nil`） | Odin（`or_return`、`or_else`） |
| 所有权模式（`read`/`mut`/`move`）在调用处显式标注 | Rust 借用检查器 |
| `safe` 环的人体工程学 | Jam（介于 Zig 和 Rust 之间的语言） |
| `base` 环的底层控制能力 | Zig + Odin |
| 异步框内核架构 | Asterinas（Rust） |

---

## 核心设计铁律

| 铁律 | 说明 |
|------|------|
| **双区架构** | 每个 `.rym` 文件物理分为定义区（上）和算法区（下）。类型和函数写在上半部，只有管道表达式写在下半部。 |
| **绝对平坦化** | 多行块禁止嵌套在其他多行块内。复杂控制流必须提取为命名函数。 |
| **管道符连接** | 算法区中 `\|>` 是唯一合法的拓扑连接符。`a \|> f(b)` 等价于 `f(a, b)`。 |
| **显式所有权** | 每个参数都必须标注 `read`（只读借用）、`mut`（可变借用）或 `move`（所有权转移）。 |
| **显式分配器** | 每个分配内存的函数必须显式接收 `Allocator` 参数，不存在隐式堆。 |
| **显式错误处理** | 无异常机制。在调用处用 `or_return`、`or_panic`、`or_else`、`or_zero`、`or_nil` 处理错误。 |
| **双环隔离** | `safe` 环：自动安全，RAII。`base` 环：直接操作硬件，内联 LoongArch 汇编，无运行时检查。 |
| **零 FFI** | 无 `extern` 关键字，无 C ABI。跨语言通信仅通过 IPC 进行。 |
| **无包管理器** | `import "路径/文件.rym"` 直接合并源码 AST 节点。标准库内置于 `rymc` 编译器中。 |

---

## 与同类语言的对比

|  | **Rym** | **C** | **Rust** | **Zig** |
|--|---------|-------|----------|---------|
| 内存模型 | 显式分配器 + 所有权模式 | 手动 `malloc`/`free` | 借用检查器 | 显式分配器 |
| 错误处理 | `or_*` 操作符 | 返回码 | `Result` + `?` | 错误联合 |
| 外部调用 | **无** | C 即 ABI | `extern "C"` | `extern` 块 |
| 包管理 | **无** | 无 | Cargo | `build.zig` |
| 中文标识符 | **原生支持** | 否 | 否 | 否 |

核心区别：Rust 和 Zig 是通用语言，恰好也适合写操作系统。Rym 是专门为一个操作系统而设计的语言——每一条铁律都服务于这个目标。

---

## 源文件扩展名

Rym 源文件使用 `.rym` 扩展名。

```
hello.rym      → rymc hello.rym → ./hello
```

---

## 编译器架构

```
源码 (.rym)
     │
     ▼
┌─────────────────────────────────────────────────┐
│                  rymc 编译器                      │
│                                                  │
│  rym-lexer ──► rym-ast ──► rym-parser            │
│                                 │                │
│                                 ▼                │
│                            rym-sema              │
│           （类型检查 + 所有权验证 + import 解析   │
│             + struct 字段布局）                   │
│                                 │                │
│                                 ▼                │
│                             rym-ir               │
│                       （SSA 三地址码）            │
│                                 │                │
│                     ┌───────────┴───────────┐    │
│                     ▼                       ▼    │
│               rym-codegen             rym-codegen│
│            （C 后端，跨平台）       （LA64 后端）  │
└─────────────────────────────────────────────────┘
          │                               │
          ▼                               ▼
   cc/clang → 本地可执行文件        as + ld → ELF
   （macOS、Linux 等任意平台）      （LoongArch64 Linux）
```

引导编译器（`rymc`）用 Rust 编写。一旦 Rym 能够编译自身，Rust 引导编译器将被替换。

---

## 仓库结构

```
compiler/
├── Cargo.toml
└── crates/
    ├── rymc/        编译器 CLI 入口与流水线驱动
    ├── rym-lexer/   词法器（支持 Unicode 和中文标识符）
    ├── rym-ast/     AST 节点定义
    ├── rym-parser/  解析器（强制绝对平坦化规则）
    ├── rym-sema/    类型检查、所有权分析、import 解析
    ├── rym-ir/      SSA IR 定义、降级 Pass、struct 布局
    └── rym-codegen/ C 后端（跨平台）+ LA64 后端
runtime/
    └── start.s      LA64 _start、系统调用封装（write/read/exit）
```

---

## 构建与使用

```bash
git clone https://github.com/silicon-bastion/rym
cd rym/compiler
cargo build --release
cargo test          # 全部 crate 共 31 个测试

# 编译 .rym 文件（C 后端，macOS/Linux 均可运行）
./target/release/rymc hello.rym

# 目标平台为 LoongArch64（需要 LA64 工具链或 QEMU）
./target/release/rymc hello.rym --target la64

# 查看中间形式
./target/release/rymc hello.rym --dump-tokens
./target/release/rymc hello.rym --dump-ast
./target/release/rymc hello.rym --dump-ir

# 只生成 C 代码，不调用编译器
./target/release/rymc hello.rym --emit-only
```

需要 Rust 1.80 及以上版本。

---

## 进度

| 模块 | 状态 |
|------|------|
| 词法器（Unicode、中文关键字） | ✅ 完成 |
| AST | ✅ 完成 |
| 解析器（平坦化规则、完整语法） | ✅ 完成 |
| 语义分析（类型、所有权、struct 字段） | ✅ 完成 |
| import 解析（多文件、循环检测） | ✅ 完成 |
| IR（SSA 三地址码、struct 布局） | ✅ 完成 |
| C 后端（跨平台，macOS/Linux） | ✅ 完成 |
| LA64 后端（LoongArch64 汇编） | 🔧 基础可用，暂无优化器 |
| 字符串 / 数组 / 矩阵 / 分配器类型 | ✅ 完成 |
| 自举 | 📋 计划中 |

---

## 相关项目

- [rymos](https://github.com/silicon-bastion/rymos) — Rym 所服务的操作系统

---

## 许可证

采用[木兰宽松许可证，第 2 版](LICENSE)（Mulan PSL v2）发布。
