use std::{result::Result, sync::Arc};

use api::v1::codec;
use datafusion::physical_plan::{
    expressions::{
        Column as DfColumn, IsNotNullExpr as DfIsNotNullExpr, IsNullExpr as DfIsNullExpr,
        NotExpr as DfNotExpr,
    },
    PhysicalExpr as DfPhysicalExpr,
};
use snafu::OptionExt;

use crate::error::{EmptyPhysicalExprSnafu, Error, MissingFieldSnafu, UnsupportedDfExprSnafu};

pub type PhysicalExprRef = Arc<dyn DfPhysicalExpr>;

// grpc -> datafusion (physical expr)
pub(crate) fn parse_grpc_physical_expr(
    proto: &codec::PhysicalExprNode,
) -> Result<PhysicalExprRef, Error> {
    let expr_type = proto.expr_type.as_ref().context(EmptyPhysicalExprSnafu {
        name: format!("{:?}", proto),
    })?;

    // TODO(fys): impl other physical expr
    let pexpr: PhysicalExprRef = match expr_type {
        codec::physical_expr_node::ExprType::Column(c) => {
            let pcol = DfColumn::new(&c.name, c.index as usize);
            Arc::new(pcol)
        }
        codec::physical_expr_node::ExprType::IsNullExpr(expr) => Arc::new(DfIsNullExpr::new(
            parse_required_physical_box_expr(&expr.expr)?,
        )),
        codec::physical_expr_node::ExprType::IsNotNullExpr(expr) => Arc::new(DfIsNotNullExpr::new(
            parse_required_physical_box_expr(&expr.expr)?,
        )),
        codec::physical_expr_node::ExprType::NotExpr(expr) => Arc::new(DfNotExpr::new(
            parse_required_physical_box_expr(&expr.expr)?,
        )),
    };
    Ok(pexpr)
}

fn parse_required_physical_box_expr(
    expr: &Option<Box<codec::PhysicalExprNode>>,
) -> Result<PhysicalExprRef, Error> {
    expr.as_ref()
        .map(|e| parse_grpc_physical_expr(e.as_ref()))
        .transpose()?
        .context(MissingFieldSnafu { field: "expr" })
}

// datafusion -> grpc (physical expr)
pub(crate) fn parse_df_physical_expr(
    df_expr: PhysicalExprRef,
) -> Result<codec::PhysicalExprNode, Error> {
    let expr = df_expr.as_any();

    // TODO(fys): impl other physical expr
    if let Some(expr) = expr.downcast_ref::<DfColumn>() {
        Ok(codec::PhysicalExprNode {
            expr_type: Some(codec::physical_expr_node::ExprType::Column(
                codec::PhysicalColumn {
                    name: expr.name().to_string(),
                    index: expr.index() as u64,
                },
            )),
        })
    } else if let Some(expr) = expr.downcast_ref::<DfIsNullExpr>() {
        let node = parse_df_physical_expr(expr.arg().to_owned())?;
        Ok(codec::PhysicalExprNode {
            expr_type: Some(codec::physical_expr_node::ExprType::IsNullExpr(Box::new(
                codec::PhysicalIsNull {
                    expr: Some(Box::new(node)),
                },
            ))),
        })
    } else if let Some(expr) = expr.downcast_ref::<DfIsNotNullExpr>() {
        let node = parse_df_physical_expr(expr.arg().to_owned())?;
        Ok(codec::PhysicalExprNode {
            expr_type: Some(codec::physical_expr_node::ExprType::IsNotNullExpr(
                Box::new(codec::PhysicalIsNotNull {
                    expr: Some(Box::new(node)),
                }),
            )),
        })
    } else if let Some(expr) = expr.downcast_ref::<DfNotExpr>() {
        let node = parse_df_physical_expr(expr.arg().to_owned())?;
        Ok(codec::PhysicalExprNode {
            expr_type: Some(codec::physical_expr_node::ExprType::NotExpr(Box::new(
                codec::PhysicalNot {
                    expr: Some(Box::new(node)),
                },
            ))),
        })
    } else {
        UnsupportedDfExprSnafu {
            name: df_expr.to_string(),
        }
        .fail()?
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::physical_plan::{
        expressions::{Column as DfColumn, IsNotNullExpr, IsNullExpr, NotExpr},
        PhysicalExpr,
    };

    use super::PhysicalExprRef;
    use crate::physical::expr::{parse_df_physical_expr, parse_grpc_physical_expr};

    #[test]
    fn test_column_expr() {
        let df_column = DfColumn::new("name", 11);
        let df_expr = Arc::new(df_column);

        roundtrip_test(df_expr, assert_eq_column);
    }

    #[test]
    fn test_is_null_expr() {
        let df_column = DfColumn::new("name", 11);
        let df_column = Arc::new(df_column);
        let df_expr = Arc::new(IsNullExpr::new(df_column));

        roundtrip_test(df_expr, |x, y| {
            let x = x.as_any().downcast_ref::<IsNullExpr>().unwrap().arg();
            let y = y.as_any().downcast_ref::<IsNullExpr>().unwrap().arg();
            assert_eq_column(x, y);
        });
    }

    #[test]
    fn test_is_not_null_expr() {
        let df_column = DfColumn::new("name", 11);
        let df_column = Arc::new(df_column);
        let df_expr = Arc::new(IsNotNullExpr::new(df_column));

        roundtrip_test(df_expr, |x, y| {
            let x = x.as_any().downcast_ref::<IsNotNullExpr>().unwrap().arg();
            let y = y.as_any().downcast_ref::<IsNotNullExpr>().unwrap().arg();
            assert_eq_column(x, y);
        });
    }

    #[test]
    fn test_not_expr() {
        let df_column = DfColumn::new("name", 11);
        let df_column = Arc::new(df_column);
        let df_expr = Arc::new(NotExpr::new(df_column));

        roundtrip_test(df_expr, |x, y| {
            let x = x.as_any().downcast_ref::<NotExpr>().unwrap().arg();
            let y = y.as_any().downcast_ref::<NotExpr>().unwrap().arg();
            assert_eq_column(x, y);
        });
    }

    fn roundtrip_test<F>(df_expr: Arc<dyn PhysicalExpr>, compare: F)
    where
        F: Fn(&PhysicalExprRef, &PhysicalExprRef),
    {
        let df_expr_clone = df_expr.clone();
        let grpc = parse_df_physical_expr(df_expr).unwrap();
        let df = parse_grpc_physical_expr(&grpc).unwrap();
        compare(&df_expr_clone, &df);
    }

    fn assert_eq_column(x: &PhysicalExprRef, y: &PhysicalExprRef) {
        assert_eq!(
            x.as_any().downcast_ref::<DfColumn>().unwrap(),
            y.as_any().downcast_ref::<DfColumn>().unwrap()
        );
    }
}
