//! AST rewriter that transforms TypeScript code into query expressions.

use crate::query::BinaryExpr as QBinaryExpr;
use crate::query::BinaryOp as QBinaryOp;
use crate::query::Expr as QExpr;
use crate::query::Filter;
use crate::query::Literal as QLiteral;
use crate::query::Operator;
use crate::query::PropertyAccessExpr;
use crate::symbols::Symbols;
use crate::transforms::filter::infer_filter;
use std::str::FromStr;
use swc_ecmascript::ast::ExportDefaultDecl;
use swc_ecmascript::ast::FnExpr;
use swc_ecmascript::ast::Function;
use swc_ecmascript::ast::ModuleDecl;
use swc_ecmascript::ast::Number;

use swc_atoms::JsWord;
use swc_common::Span;
use swc_ecmascript::ast::{
    ArrowExpr, AwaitExpr, BlockStmt, BlockStmtOrExpr, Bool, CallExpr, Callee, Decl, DefaultDecl,
    Expr, ExprOrSpread, ExprStmt, Ident, KeyValueProp, Lit, MemberExpr, MemberProp, Module,
    ModuleItem, ObjectLit, Prop, PropName, PropOrSpread, Stmt, Str, Super, VarDecl, VarDeclarator,
};

/// The query language target
#[derive(Clone)]
pub enum Target {
    /// Emit JavaScript using ChiselStrike query expressions.
    JavaScript,
    /// Emit TypeScript using ChiselStrike query expressions.
    TypeScript,
}

type TargetParseError = &'static str;

impl FromStr for Target {
    type Err = TargetParseError;
    fn from_str(target: &str) -> Result<Self, Self::Err> {
        match target {
            "js" => Ok(Target::JavaScript),
            "ts" => Ok(Target::TypeScript),
            _ => Err("Unknown target"),
        }
    }
}

pub struct Rewriter {
    target: Target,
    symbols: Symbols,
}

impl Rewriter {
    pub fn new(target: Target, symbols: Symbols) -> Self {
        Self { target, symbols }
    }

    pub fn rewrite(&self, module: Module) -> Module {
        let mut body = Vec::new();
        for item in module.body {
            body.push(self.rewrite_item(&item));
        }
        Module {
            span: module.span,
            body,
            shebang: module.shebang,
        }
    }

    fn rewrite_item(&self, item: &ModuleItem) -> ModuleItem {
        match item {
            ModuleItem::ModuleDecl(decl) => {
                let decl = self.rewrite_module_decl(decl);
                ModuleItem::ModuleDecl(decl)
            }
            ModuleItem::Stmt(stmt) => {
                let stmt = self.rewrite_stmt(stmt);
                ModuleItem::Stmt(stmt)
            }
        }
    }

    fn rewrite_module_decl(&self, module_decl: &ModuleDecl) -> ModuleDecl {
        match module_decl {
            ModuleDecl::ExportDefaultDecl(ExportDefaultDecl {
                span,
                decl: DefaultDecl::Fn(fn_expr),
            }) => {
                let fn_expr = self.rewrite_fn_expr(fn_expr);
                ModuleDecl::ExportDefaultDecl(ExportDefaultDecl {
                    span: *span,
                    decl: DefaultDecl::Fn(fn_expr),
                })
            }
            _ => module_decl.clone(),
        }
    }

    fn rewrite_fn_expr(&self, fn_expr: &FnExpr) -> FnExpr {
        let body = fn_expr
            .function
            .body
            .as_ref()
            .map(|body| self.rewrite_block_stmt(body));
        FnExpr {
            ident: fn_expr.ident.clone(),
            function: Function {
                params: fn_expr.function.params.clone(),
                decorators: fn_expr.function.decorators.clone(),
                span: fn_expr.function.span,
                body,
                is_generator: fn_expr.function.is_generator,
                is_async: fn_expr.function.is_async,
                type_params: fn_expr.function.type_params.clone(),
                return_type: fn_expr.function.return_type.clone(),
            },
        }
    }

    fn rewrite_stmt(&self, stmt: &Stmt) -> Stmt {
        match stmt {
            Stmt::Decl(decl) => {
                let decl = self.rewrite_decl(decl);
                Stmt::Decl(decl)
            }
            Stmt::Expr(expr_stmt) => {
                let expr = self.rewrite_expr(&*expr_stmt.expr);
                let expr_stmt = ExprStmt {
                    span: expr_stmt.span,
                    expr: Box::new(expr),
                };
                Stmt::Expr(expr_stmt)
            }
            _ => stmt.clone(),
        }
    }

    fn rewrite_decl(&self, decl: &Decl) -> Decl {
        match decl {
            Decl::Var(var_decl) => {
                let mut decls = Vec::new();
                for decl in &var_decl.decls {
                    let decl = self.rewrite_var_declarator(decl);
                    decls.push(decl);
                }
                Decl::Var(VarDecl {
                    span: var_decl.span,
                    kind: var_decl.kind,
                    declare: var_decl.declare,
                    decls,
                })
            }
            _ => decl.clone(),
        }
    }

    fn rewrite_var_declarator(&self, var_declarator: &VarDeclarator) -> VarDeclarator {
        let init = var_declarator
            .init
            .as_ref()
            .map(|init| Box::new(self.rewrite_expr(init)));
        VarDeclarator {
            span: var_declarator.span,
            name: var_declarator.name.clone(),
            init,
            definite: var_declarator.definite,
        }
    }

    fn rewrite_expr(&self, expr: &Expr) -> Expr {
        match expr {
            Expr::Arrow(arrow_expr) => {
                let arrow_expr = self.rewrite_arrow_expr(arrow_expr);
                Expr::Arrow(arrow_expr)
            }
            Expr::Await(await_expr) => {
                let await_expr = self.rewrite_await_expr(await_expr);
                Expr::Await(await_expr)
            }
            Expr::Call(call_expr) => {
                let call_expr = self.rewrite_call_expr(call_expr);
                Expr::Call(call_expr)
            }
            Expr::Member(member_expr) => {
                let member_expr = self.rewrite_member_expr(member_expr);
                Expr::Member(member_expr)
            }
            _ => expr.clone(),
        }
    }

    fn rewrite_arrow_expr(&self, arrow_expr: &ArrowExpr) -> ArrowExpr {
        let body = match &arrow_expr.body {
            BlockStmtOrExpr::BlockStmt(block_stmt) => {
                let block_stmt = self.rewrite_block_stmt(block_stmt);
                BlockStmtOrExpr::BlockStmt(block_stmt)
            }
            BlockStmtOrExpr::Expr(expr) => {
                let expr = self.rewrite_expr(expr);
                BlockStmtOrExpr::Expr(Box::new(expr))
            }
        };
        ArrowExpr {
            span: arrow_expr.span,
            params: arrow_expr.params.clone(),
            body,
            is_async: arrow_expr.is_async,
            is_generator: arrow_expr.is_generator,
            type_params: arrow_expr.type_params.clone(),
            return_type: arrow_expr.return_type.clone(),
        }
    }

    fn rewrite_block_stmt(&self, block_stmt: &BlockStmt) -> BlockStmt {
        let mut stmts = vec![];
        for stmt in &block_stmt.stmts {
            stmts.push(self.rewrite_stmt(stmt));
        }
        BlockStmt {
            span: block_stmt.span,
            stmts,
        }
    }

    fn rewrite_await_expr(&self, await_expr: &AwaitExpr) -> AwaitExpr {
        AwaitExpr {
            span: await_expr.span,
            arg: Box::new(self.rewrite_expr(&await_expr.arg)),
        }
    }

    fn rewrite_callee(&self, callee: &Callee) -> Callee {
        match callee {
            Callee::Super(Super { span }) => Callee::Super(Super { span: *span }),
            Callee::Import(import) => Callee::Import(*import),
            Callee::Expr(expr) => Callee::Expr(Box::new(self.rewrite_expr(expr))),
        }
    }

    fn rewrite_expr_or_spread(&self, expr_or_spread: &ExprOrSpread) -> ExprOrSpread {
        let expr = self.rewrite_expr(&*expr_or_spread.expr);
        ExprOrSpread {
            spread: expr_or_spread.spread,
            expr: Box::new(expr),
        }
    }

    fn rewrite_call_expr(&self, call_expr: &CallExpr) -> CallExpr {
        if let Some(filter) = infer_filter(call_expr, &self.symbols) {
            match self.target {
                Target::JavaScript | Target::TypeScript => {
                    return self.to_ts_expr(call_expr, &filter);
                }
            }
        }
        let args = call_expr
            .args
            .iter()
            .map(|expr| self.rewrite_expr_or_spread(expr))
            .collect();
        CallExpr {
            span: call_expr.span,
            callee: self.rewrite_callee(&call_expr.callee),
            args,
            type_args: call_expr.type_args.clone(),
        }
    }

    fn to_ts_expr(&self, call_expr: &CallExpr, filter: &Operator) -> CallExpr {
        match filter {
            Operator::Filter(filter) => {
                let callee = self.rewrite_filter_callee(&call_expr.callee);
                let expr = self.filter_to_ts(filter, call_expr.span);
                let expr = ExprOrSpread {
                    spread: None,
                    expr: Box::new(expr),
                };
                let mut args = call_expr.args.clone();
                args.push(expr);
                CallExpr {
                    span: call_expr.span,
                    callee,
                    args,
                    type_args: call_expr.type_args.clone(),
                }
            }
            _ => {
                todo!("TypeScript target only supports filtering.");
            }
        }
    }

    /// Rewrites the filter() call with __filterWithExpression().
    fn rewrite_filter_callee(&self, callee: &Callee) -> Callee {
        match callee {
            Callee::Expr(expr) => match &**expr {
                Expr::Member(member_expr) => {
                    let mut member_expr = member_expr.clone();
                    let prop = MemberProp::Ident(Ident {
                        span: member_expr.span,
                        sym: JsWord::from("__filterWithExpression"),
                        optional: false,
                    });
                    member_expr.prop = prop;
                    Callee::Expr(Box::new(Expr::Member(member_expr)))
                }
                _ => {
                    todo!();
                }
            },
            _ => {
                todo!();
            }
        }
    }

    fn filter_to_ts(&self, filter: &Filter, span: Span) -> Expr {
        self.expr_to_ts(&filter.predicate, &filter.parameters, span)
    }

    fn expr_to_ts(&self, expr: &QExpr, params: &[String], span: Span) -> Expr {
        match expr {
            QExpr::BinaryExpr(binary_expr) => self.binary_expr_to_ts(binary_expr, params, span),
            QExpr::PropertyAccess(property_access_expr) => {
                self.property_access_to_ts(property_access_expr, params, span)
            }
            QExpr::Identifier(ident) => self.identifier_to_ts(ident, params, span),
            QExpr::Literal(lit) => self.literal_to_ts(lit, span),
        }
    }

    fn binary_expr_to_ts(&self, binary_expr: &QBinaryExpr, params: &[String], span: Span) -> Expr {
        let mut props = vec![make_expr_type("Binary", span)];
        let left = self.expr_to_ts(&binary_expr.left, params, span);
        let left = PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
            key: PropName::Ident(Ident {
                span,
                sym: JsWord::from("left"),
                optional: false,
            }),
            value: Box::new(left),
        })));
        props.push(left);
        let op = self.binary_op_to_ts(&binary_expr.op, span);
        let op = PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
            key: PropName::Ident(Ident {
                span,
                sym: JsWord::from("op"),
                optional: false,
            }),
            value: Box::new(op),
        })));
        props.push(op);
        let right = self.expr_to_ts(&binary_expr.right, params, span);
        let right = PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
            key: PropName::Ident(Ident {
                span,
                sym: JsWord::from("right"),
                optional: false,
            }),
            value: Box::new(right),
        })));
        props.push(right);
        Expr::Object(ObjectLit { span, props })
    }

    fn binary_op_to_ts(&self, binary_op: &QBinaryOp, span: Span) -> Expr {
        let raw_op = match binary_op {
            QBinaryOp::And => "And",
            QBinaryOp::Eq => "Eq",
            QBinaryOp::Gt => "Gt",
            QBinaryOp::GtEq => "GtEq",
            QBinaryOp::Lt => "Lt",
            QBinaryOp::LtEq => "LtEq",
            QBinaryOp::NotEq => "NotEq",
            QBinaryOp::Or => "Or",
        };
        make_str_lit(raw_op, span)
    }

    fn property_access_to_ts(
        &self,
        property_access_expr: &PropertyAccessExpr,
        params: &[String],
        span: Span,
    ) -> Expr {
        let mut props = vec![make_expr_type("Property", span)];
        let obj = PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
            key: PropName::Ident(Ident {
                span,
                sym: JsWord::from("object"),
                optional: false,
            }),
            value: Box::new(self.expr_to_ts(&property_access_expr.object, params, span)),
        })));
        props.push(obj);
        let prop = PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
            key: PropName::Ident(Ident {
                span,
                sym: JsWord::from("property"),
                optional: false,
            }),
            value: Box::new(make_str_lit(&property_access_expr.property, span)),
        })));
        props.push(prop);
        Expr::Object(ObjectLit { span, props })
    }

    fn identifier_to_ts(&self, ident: &str, params: &[String], span: Span) -> Expr {
        let mut props = vec![];
        if let Some(pos) = params.iter().position(|param| param == ident) {
            props.push(make_expr_type("Parameter", span));
            let lit = PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                key: PropName::Ident(Ident {
                    span,
                    sym: JsWord::from("position"),
                    optional: false,
                }),
                value: Box::new(make_num_lit(&(pos as f64), span)),
            })));
            props.push(lit);
        } else {
            props.push(make_expr_type("Identifier", span));
            let lit = PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                key: PropName::Ident(Ident {
                    span,
                    sym: JsWord::from("ident"),
                    optional: false,
                }),
                value: Box::new(make_str_lit(ident, span)),
            })));
            props.push(lit);
        }
        Expr::Object(ObjectLit { span, props })
    }

    fn literal_to_ts(&self, lit: &QLiteral, span: Span) -> Expr {
        let mut props = vec![make_expr_type("Literal", span)];
        let lit = match lit {
            QLiteral::Bool(v) => make_bool_lit(*v, span),
            QLiteral::Str(s) => make_str_lit(s, span),
            QLiteral::Num(n) => make_num_lit(n, span),
        };
        let lit = PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
            key: PropName::Ident(Ident {
                span,
                sym: JsWord::from("value"),
                optional: false,
            }),
            value: Box::new(lit),
        })));
        props.push(lit);
        Expr::Object(ObjectLit { span, props })
    }

    fn rewrite_member_expr(&self, member_expr: &MemberExpr) -> MemberExpr {
        MemberExpr {
            span: member_expr.span,
            obj: Box::new(self.rewrite_expr(&member_expr.obj)),
            prop: self.rewrite_member_prop(&member_expr.prop),
        }
    }

    fn rewrite_member_prop(&self, member_prop: &MemberProp) -> MemberProp {
        /* FIXME: Computed property names have expressions */
        member_prop.clone()
    }
}

fn make_expr_type(expr_type: &str, span: Span) -> PropOrSpread {
    PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
        key: PropName::Ident(Ident {
            span,
            sym: JsWord::from("exprType"),
            optional: false,
        }),
        value: Box::new(make_str_lit(expr_type, span)),
    })))
}

fn make_bool_lit(value: bool, span: Span) -> Expr {
    Expr::Lit(Lit::Bool(Bool { span, value }))
}

fn make_str_lit(raw_str: &str, span: Span) -> Expr {
    Expr::Lit(Lit::Str(Str {
        span,
        value: JsWord::from(raw_str),
        raw: None,
    }))
}

fn make_num_lit(num: &f64, span: Span) -> Expr {
    Expr::Lit(Lit::Num(Number {
        span,
        value: *num,
        raw: None,
    }))
}
