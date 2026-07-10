use std::collections::{HashMap, HashSet};
use rym_lexer::Span;
use rym_ast::expr::OwnershipMode;

/// Resolved type — a simplified representation used during sema.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedTy {
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64,
    Bool,
    Usize,
    Str,
    Void,
    Slice(Box<ResolvedTy>),
    Ptr(Box<ResolvedTy>),
    PtrMut(Box<ResolvedTy>),
    Result(Box<ResolvedTy>, Box<ResolvedTy>),
    Option(Box<ResolvedTy>),
    Allocator,
    /// Fixed-size stack array: `[4]i32`
    Array { size: usize, elem: Box<ResolvedTy> },
    /// User-defined named type.
    Named(String),
    /// Function pointer: `fn(T, U) -> R`
    FnPtr { params: Vec<ResolvedTy>, ret: Box<ResolvedTy> },
    /// Not yet resolved — used as a placeholder before inference.
    Unknown,
}

impl ResolvedTy {
    pub fn is_result(&self) -> bool {
        matches!(self, ResolvedTy::Result(_, _))
    }

    pub fn display(&self) -> String {
        match self {
            ResolvedTy::I8    => "i8".into(),
            ResolvedTy::I16   => "i16".into(),
            ResolvedTy::I32   => "i32".into(),
            ResolvedTy::I64   => "i64".into(),
            ResolvedTy::U8    => "u8".into(),
            ResolvedTy::U16   => "u16".into(),
            ResolvedTy::U32   => "u32".into(),
            ResolvedTy::U64   => "u64".into(),
            ResolvedTy::F32   => "f32".into(),
            ResolvedTy::F64   => "f64".into(),
            ResolvedTy::Bool  => "bool".into(),
            ResolvedTy::Usize => "usize".into(),
            ResolvedTy::Str   => "str".into(),
            ResolvedTy::Void  => "void".into(),
            ResolvedTy::Allocator       => "Allocator".into(),
            ResolvedTy::Array { size, elem } => format!("[{}]{}", size, elem.display()),
            ResolvedTy::Named(n)        => n.clone(),
            ResolvedTy::Slice(t)        => format!("[]{}", t.display()),
            ResolvedTy::Ptr(t)          => format!("*{}", t.display()),
            ResolvedTy::PtrMut(t)       => format!("*mut {}", t.display()),
            ResolvedTy::Result(ok, err) => format!("Result({}, {})", ok.display(), err.display()),
            ResolvedTy::Option(t)       => format!("Option({})", t.display()),
            ResolvedTy::FnPtr { params, ret } => {
                let ps = params.iter().map(|p| p.display()).collect::<Vec<_>>().join(", ");
                format!("fn({}) -> {}", ps, ret.display())
            }
            ResolvedTy::Unknown         => "<unknown>".into(),
        }
    }
}

/// A binding in the current scope.
#[derive(Debug, Clone)]
pub struct Binding {
    pub ty:      ResolvedTy,
    pub mode:    OwnershipMode,
    /// Whether the binding is mutable (declared with `设`).
    pub mutable: bool,
    /// Whether the value has been moved out.
    pub moved:   bool,
    pub span:    Span,
}

/// A function signature stored in the function table.
#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<(String, OwnershipMode, ResolvedTy)>,
    pub ret:    ResolvedTy,
}

/// Lexical scope stack.
pub struct Scope {
    /// Stack of binding maps; innermost last.
    frames: Vec<HashMap<String, Binding>>,
    /// Global function signatures.
    pub fns: HashMap<String, FnSig>,
    /// User-defined type field maps.
    pub types: HashMap<String, Vec<(String, ResolvedTy)>>,
    /// Names of enum types (used to recognize `EnumName.Variant` expressions).
    pub enums: HashSet<String>,
}

impl Scope {
    pub fn new() -> Self {
        Self {
            frames: vec![HashMap::new()],
            fns:    HashMap::new(),
            types:  HashMap::new(),
            enums:  HashSet::new(),
        }
    }

    pub fn push(&mut self) {
        self.frames.push(HashMap::new());
    }

    pub fn pop(&mut self) {
        self.frames.pop();
    }

    pub fn define(&mut self, name: String, binding: Binding) {
        self.frames.last_mut().unwrap().insert(name, binding);
    }

    pub fn lookup(&self, name: &str) -> Option<&Binding> {
        for frame in self.frames.iter().rev() {
            if let Some(b) = frame.get(name) {
                return Some(b);
            }
        }
        None
    }

    pub fn lookup_mut(&mut self, name: &str) -> Option<&mut Binding> {
        for frame in self.frames.iter_mut().rev() {
            if frame.contains_key(name) {
                return frame.get_mut(name);
            }
        }
        None
    }

    /// Mark a binding as moved — further uses are a compile error.
    pub fn mark_moved(&mut self, name: &str) {
        if let Some(b) = self.lookup_mut(name) {
            b.moved = true;
        }
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}
