# Rym

> A systems programming language for LoongArch64, built to power the [Rymos](https://github.com/silicon-bastion/rymos) operating system.

[![CI](https://github.com/silicon-bastion/rym/actions/workflows/ci.yml/badge.svg)](https://github.com/silicon-bastion/rym/actions)
[![License: Mulan PSL v2](https://img.shields.io/badge/license-Mulan%20PSL%20v2-blue.svg)](LICENSE)
[![中文](https://img.shields.io/badge/文档-中文-red.svg)](README.zh.md)

---

## What is Rym?

Rym is a small, opinionated systems language with three goals:

1. **Absolute clarity** — every ownership transfer, allocation, and error path is written explicitly. No implicit behaviour.
2. **Zero hidden layers** — no GC, no runtime exceptions, no implicit ABI, no package manager.
3. **LoongArch64 first** — designed for Loongson processors from day one, with no x86 legacy baggage.

Rym is intentionally niche. It is not trying to replace Rust or C. It exists to write one operating system well.

---

## Language at a glance

```rym
// Definition zone — all types and functions declared first
type Request {
    path: str
    method: str
    body: []u8
}

fn parse(raw: read []u8, alloc: read Allocator) -> Result(Request, str) {
    定 path   = extract_path(raw).or_return?
    定 method = extract_method(raw).or_else("GET")
    定 body   = alloc.alloc(u8, raw.length).or_zero
    return Ok(Request{ path=path, method=method, body=body })
}

fn route(req: read Request, alloc: read Allocator) -> Result(Response, str) {
    定 content = alloc.alloc(u8, 256).or_panic("route must succeed")
    copy(content, "Hello from Rym!")
    return Ok(Response{ status=200, content=content })
}

fn send(res: read Response, conn: Connection) -> Result(void, str) {
    定 written = syscall(write, conn.fd, res.content.ptr, res.content.len)
    if written != res.content.len { return Err("incomplete send") }
    return Ok(void{})
}

// Algorithm zone — pipeline topology only, no definitions
定 alloc = SystemAllocator{}
定 conn  = accept()

conn.recv()
    |> parse(alloc)
    |> route(alloc)
    |> send(conn)
```

---

## What makes Rym different

### Live node editor (like Unity)
The `rym` toolchain ships a visual editor where the **pipeline graph and AST tree are rendered in real time** as you type. Every `|>` pipe connection, every ownership mode, every function boundary appears as a live node graph — not a static diagram. The editor is itself written in Rym and fully customisable: you can rearrange panels, add your own views, and theme the entire UI.

### Two rings, two personalities

| | `base` ring | `safe` ring |
|-|-------------|-------------|
| **Closest analogue** | Zig + Odin | Zig + Jam |
| **Memory** | Manual, explicit SOA layout, zero-cost abstractions | RAII + explicit allocator |
| **Safety** | None — developer has full control | Compiler-enforced ownership |
| **Hardware** | Inline LoongArch64 asm, direct syscall, MMIO | Standard library IPC interfaces |
| **Use case** | Compiler extensions, OS kernel, device drivers | Application logic, OS services |

> *Jam* is a language positioned between Zig and Rust — ergonomic error handling without the full borrow checker. The `safe` ring adopts the same philosophy.

### Chinese-first toolchain
All compiler error messages, the editor UI, and the standard library documentation are available in Chinese. Chinese identifiers are first-class — `fn 解析请求(原始: read []u8)` is valid Rym. The two reserved Chinese keywords `定` (immutable) and `设` (mutable) mirror idiomatic Chinese rather than borrowing from English.

### Design influences

| Feature | Inspired by |
|---------|-------------|
| Explicit allocator parameter | Odin, Zig |
| SOA (Structure of Arrays) data layout | Odin |
| Value types — structs passed by value, no hidden indirection | MoonBit |
| Error-handling operators (`or_return`, `or_panic`, `or_else`, `or_zero`, `or_nil`) | Odin (`or_return`, `or_else`) |
| Ownership modes (`read`/`mut`/`move`) | Rust borrow checker — explicit at call site |
| `safe` ring ergonomics | Jam (between Zig and Rust) |
| `base` ring low-level control | Zig + Odin |
| Async frame kernel | Asterinas (Rust) |

---

## Key design rules

| Rule | Description |
|------|-------------|
| **Dual-zone files** | Every `.rym` file is split into a *definition zone* (top) and an *algorithm zone* (bottom). Types and functions go up top; only pipeline expressions go at the bottom. |
| **No nesting** | Multi-line blocks cannot nest inside other multi-line blocks. Complex control flow must be extracted into a named function. |
| **`\|>` is the only connector** | In the algorithm zone, `\|>` is the only way to chain operations. `a \|> f(b)` means `f(a, b)`. |
| **Explicit ownership** | Every parameter is annotated `read` (shared borrow), `mut` (exclusive borrow), or `move` (ownership transfer). |
| **Explicit allocator** | Every function that allocates memory takes an `Allocator` parameter. There is no implicit heap. |
| **Explicit errors** | No exceptions. Errors are handled at the call site with `or_return`, `or_panic`, `or_else`, `or_zero`, or `or_nil`. |
| **Two rings** | `safe` ring: automatic safety, RAII. `base` ring: raw hardware access, inline LoongArch asm, no runtime checks. |
| **Zero FFI** | No `extern`, no C ABI. Cross-language communication via IPC only. |
| **No package manager** | `import "path/to/file.rym"` merges AST nodes directly. The stdlib is built into `rymc`. |

---

## How Rym compares

|  | **Rym** | **C** | **Rust** | **Zig** |
|--|---------|-------|----------|---------|
| Memory model | Explicit allocator + ownership modes | Manual `malloc`/`free` | Borrow checker | Explicit allocator |
| Error handling | `or_*` operators | Return codes | `Result` + `?` | Error unions |
| FFI | **None** | C is the ABI | `extern "C"` | `extern` blocks |
| Package manager | **None** | None | Cargo | `build.zig` |
| Chinese identifiers | **Native support** | No | No | No |

The key difference: Rust and Zig are general-purpose languages that happen to work well for OS development. Rym is a language designed for exactly one OS — every rule exists to serve that goal.

---

## Source files

Rym source files use the `.rym` extension.

```
hello.rym      → rymc hello.rym → ./hello
```

---

## Compiler architecture

```
Source (.rym)
     │
     ▼
┌─────────────────────────────────────────────────┐
│                  rymc compiler                   │
│                                                  │
│  rym-lexer ──► rym-ast ──► rym-parser            │
│                                 │                │
│                                 ▼                │
│                            rym-sema              │
│                  (type check + ownership + import│
│                   resolution + struct layout)    │
│                                 │                │
│                                 ▼                │
│                             rym-ir               │
│                      (SSA three-address code)    │
│                                 │                │
│                     ┌───────────┴───────────┐    │
│                     ▼                       ▼    │
│               rym-codegen             rym-codegen│
│            (C backend, any OS)    (LA64 backend) │
└─────────────────────────────────────────────────┘
          │                               │
          ▼                               ▼
   cc/clang → native binary      as + ld → ELF
   (macOS, Linux, any platform)  (LoongArch64 Linux)
```

The bootstrap compiler (`rymc`) is written in Rust. Once Rym can compile itself, the Rust bootstrap is retired.

---

## Repository layout

```
compiler/
├── Cargo.toml
└── crates/
    ├── rymc/        CLI entry point and pipeline driver
    ├── rym-lexer/   tokeniser (Unicode-aware, Chinese identifiers)
    ├── rym-ast/     AST node definitions
    ├── rym-parser/  token stream → AST (flatness enforcement)
    ├── rym-sema/    type checking, ownership analysis, import resolution
    ├── rym-ir/      SSA IR definitions, lowering pass, struct layouts
    └── rym-codegen/ C backend (cross-platform) + LA64 backend
runtime/
    └── start.s      LA64 _start, syscall wrappers (write/read/exit)
```

---

## Build and usage

```bash
git clone https://github.com/silicon-bastion/rym
cd rym/compiler
cargo build --release
cargo test          # 31 tests across all crates

# Compile a .rym file (C backend, runs on macOS/Linux)
./target/release/rymc hello.rym

# Target LoongArch64 (requires loongarch64 toolchain or QEMU)
./target/release/rymc hello.rym --target la64

# Inspect intermediate forms
./target/release/rymc hello.rym --dump-tokens
./target/release/rymc hello.rym --dump-ast
./target/release/rymc hello.rym --dump-ir

# Emit generated C without compiling
./target/release/rymc hello.rym --emit-only
```

Requires Rust 1.80+.

---

## Status

| Component | Status |
|-----------|--------|
| Lexer (Unicode, Chinese keywords) | ✅ Complete |
| AST | ✅ Complete |
| Parser (flatness rule, all syntax) | ✅ Complete |
| Semantic analysis (types, ownership, struct fields) | ✅ Complete |
| Import resolution (multi-file, cycle detection) | ✅ Complete |
| IR (SSA three-address, struct layouts) | ✅ Complete |
| C backend (cross-platform, macOS/Linux) | ✅ Complete |
| LA64 backend (LoongArch64 assembly) | 🔧 Basic — register alloc, no optimiser |
| String / array / matrix / allocator types | ✅ Complete |
| Self-hosting | 📋 Planned |

---

## Related

- [rymos](https://github.com/silicon-bastion/rymos) — the OS Rym is built to write

---

## License

Licensed under the [Mulan Permissive Software License, Version 2](LICENSE) (Mulan PSL v2).
