use std::{collections::HashMap, sync::Arc};

use common_error::DaftResult;
use common_treenode::{DynTreeNode, Transformed, TreeNode};
use daft_core::prelude::*;
use daft_dsl::{
    is_udf,
    optimization::{get_required_columns, replace_columns_with_expressions, requires_computation},
    resolved_col, Column, Expr, ExprRef, ResolvedColumn,
};
use indexmap::IndexSet;
use itertools::Itertools;

use super::OptimizerRule;
use crate::{
    ops::{Aggregate, Join, Pivot, Project, Source, UDFProject},
    source_info::SourceInfo,
    LogicalPlan, LogicalPlanRef,
};

#[derive(Default, Debug)]
pub struct PushDownProjection {}

impl PushDownProjection {
    pub fn new() -> Self {
        Self {}
    }

    fn try_optimize_project(
        &self,
        projection: &Project,
        plan: Arc<LogicalPlan>,
    ) -> DaftResult<Transformed<Arc<LogicalPlan>>> {
        let upstream_plan = &projection.input;
        let upstream_schema = upstream_plan.schema();

        // First, drop this projection if it is a no-op
        // (selecting exactly all parent columns in the same order and nothing else).
        let projection_is_noop = {
            // Short circuit early if the projection length is different (obviously not a no-op).
            upstream_schema.names().len() == projection.projection.len()
                && projection
                    .projection
                    .iter()
                    .zip(upstream_schema.names().iter())
                    .all(|(expr, upstream_col)| match expr.as_ref() {
                        Expr::Column(Column::Resolved(ResolvedColumn::Basic(colname))) => {
                            colname.as_ref() == upstream_col
                        }
                        _ => false,
                    })
        };
        if projection_is_noop {
            // Projection discarded but new root node has not been looked at;
            // look at the new root node.
            let new_plan = self
                .try_optimize_node(upstream_plan.clone())?
                .or(Transformed::yes(upstream_plan.clone()));
            return Ok(new_plan);
        }

        // Next, check if the upstream is another projection we can merge with.
        // This is possible iff the upstream projection's computation-required columns
        // are each only used once in this downstream projection.
        if let LogicalPlan::Project(upstream_projection) = upstream_plan.as_ref() {
            // Get all the computation-required columns from the upstream projection.
            let upstream_computations = upstream_projection
                .projection
                .iter()
                .filter_map(|e| {
                    e.input_mapping().map_or_else(
                        // None means computation required -> Some(colname)
                        || Some(e.name().to_string()),
                        // Some(computation not required) -> None
                        |_| None,
                    )
                })
                .collect::<IndexSet<_>>();

            // For each of them, make sure they are used only once in this downstream projection.
            let mut exprs_to_walk: Vec<Arc<Expr>> = projection.projection.clone();

            let mut upstream_computations_used = IndexSet::new();
            let mut okay_to_merge = true;

            while !exprs_to_walk.is_empty() {
                exprs_to_walk = exprs_to_walk
                    .iter()
                    .flat_map(|expr| {
                        // If it's a reference for a column that requires computation,
                        // record it.
                        if okay_to_merge
                            && let Expr::Column(Column::Resolved(ResolvedColumn::Basic(name))) =
                                expr.as_ref()
                            && upstream_computations.contains(name.as_ref())
                        {
                            okay_to_merge = okay_to_merge
                                && upstream_computations_used.insert(name.to_string());
                        }
                        if okay_to_merge {
                            expr.children()
                        } else {
                            // Short circuit to avoid continuing walking the tree.
                            vec![]
                        }
                    })
                    .collect();
            }

            // If the upstream is okay to merge into the current projection,
            // do the merge.
            if okay_to_merge {
                // Get the name and expression for each of the upstream columns.
                let upstream_names_to_exprs = upstream_projection
                    .projection
                    .iter()
                    .map(|e| (e.name().to_string(), e.clone()))
                    .collect::<HashMap<_, _>>();

                // Merge the projections by applying the upstream expression substitutions
                // to the current projection.
                let merged_projection = projection
                    .projection
                    .iter()
                    .map(|e| replace_columns_with_expressions(e.clone(), &upstream_names_to_exprs))
                    .collect();

                // Make a new projection node with the merged projections.
                let new_plan: LogicalPlan =
                    Project::try_new(upstream_projection.input.clone(), merged_projection)?.into();
                let new_plan: Arc<LogicalPlan> = new_plan.into();

                // Root node is changed, look at it again.
                let new_plan = self
                    .try_optimize_node(new_plan.clone())?
                    .or(Transformed::yes(new_plan));
                return Ok(new_plan);
            }
        }

        match upstream_plan.as_ref() {
            LogicalPlan::Source(source) => {
                // Prune unnecessary columns directly from the source.
                let [required_columns] = &plan.required_columns()[..] else {
                    unreachable!()
                };
                match source.source_info.as_ref() {
                    SourceInfo::Physical(external_info) => {
                        if required_columns.len() < upstream_schema.names().len() {
                            let pruned_upstream_schema = upstream_schema
                                .into_iter()
                                .filter(|field| required_columns.contains(&field.name))
                                .cloned()
                                .collect::<Vec<_>>();
                            let schema = Schema::new(pruned_upstream_schema);
                            let new_source: LogicalPlan = Source::new(
                                schema.into(),
                                Arc::new(SourceInfo::Physical(external_info.with_pushdowns(
                                    external_info.pushdowns.with_columns(Some(Arc::new(
                                        required_columns.iter().cloned().collect(),
                                    ))),
                                ))),
                            )
                            .into();
                            let new_plan = Arc::new(plan.with_new_children(&[new_source.into()]));
                            // Retry optimization now that the upstream node is different.
                            let new_plan = self
                                .try_optimize_node(new_plan.clone())?
                                .or(Transformed::yes(new_plan));
                            Ok(new_plan)
                        } else {
                            Ok(Transformed::no(plan))
                        }
                    }
                    SourceInfo::InMemory(_) => Ok(Transformed::no(plan)),
                    SourceInfo::PlaceHolder(..) => {
                        panic!("PlaceHolderInfo should not exist for optimization!");
                    }
                }
            }
            LogicalPlan::Project(upstream_projection) => {
                // Prune columns from the child projection that are not used in this projection.
                let required_columns = &plan.required_columns()[0];
                if required_columns.len() < upstream_schema.names().len() {
                    let pruned_upstream_projections = upstream_projection
                        .projection
                        .iter()
                        .filter(|&e| required_columns.contains(e.name()))
                        .cloned()
                        .collect::<Vec<_>>();

                    let new_upstream: LogicalPlan = Project::try_new(
                        upstream_projection.input.clone(),
                        pruned_upstream_projections,
                    )?
                    .into();

                    let new_plan = Arc::new(plan.with_new_children(&[new_upstream.into()]));
                    // Retry optimization now that the upstream node is different.
                    let new_plan = self
                        .try_optimize_node(new_plan.clone())?
                        .or(Transformed::yes(new_plan));
                    Ok(new_plan)
                } else {
                    Ok(Transformed::no(plan))
                }
            }
            LogicalPlan::Aggregate(aggregate) => {
                // Prune unnecessary columns from the child aggregate.
                let required_columns = &plan.required_columns()[0];
                let pruned_aggregate_exprs = aggregate
                    .aggregations
                    .iter()
                    .filter(|&e| required_columns.contains(e.name()))
                    .cloned()
                    .collect::<Vec<_>>();

                if pruned_aggregate_exprs.len() < aggregate.aggregations.len() {
                    let new_upstream: LogicalPlan = Aggregate::try_new(
                        aggregate.input.clone(),
                        pruned_aggregate_exprs,
                        aggregate.groupby.clone(),
                    )?
                    .into();

                    let new_plan = Arc::new(plan.with_new_children(&[new_upstream.into()]));
                    // Retry optimization now that the upstream node is different.
                    let new_plan = self
                        .try_optimize_node(new_plan.clone())?
                        .or(Transformed::yes(new_plan));
                    Ok(new_plan)
                } else {
                    Ok(Transformed::no(plan))
                }
            }
            LogicalPlan::UDFProject(upstream_actor_pool_projection) => {
                let required_columns = &plan.required_columns()[0];
                if !required_columns.contains(upstream_actor_pool_projection.project.name()) {
                    // We don't need the UDFProject, just convert to a regular project
                    let new_plan = LogicalPlan::Project(Project::try_new(
                        upstream_actor_pool_projection.input.clone(),
                        upstream_actor_pool_projection.passthrough_columns.clone(),
                    )?)
                    .arced();
                    let new_plan = Arc::new(plan.with_new_children(&[new_plan.into()]));

                    // Retry optimization now that the upstream node is different.
                    let new_plan = self
                        .try_optimize_node(new_plan.clone())?
                        .or(Transformed::yes(new_plan));
                    return Ok(new_plan);
                }

                // Attempt to merge the current Projection into the upstream UDFProject
                // if there aren't any actual computations being performed in the Projection, and
                // if each upstream column is used only once (no common subtrees)
                if projection
                    .projection
                    .iter()
                    .all(|e| !requires_computation(e))
                {
                    // Only perform this optimization if all required column names are distinct
                    let required_column_names = projection
                        .projection
                        .iter()
                        .flat_map(get_required_columns)
                        .collect_vec();
                    let mut all_required_column_names_distinct = true;
                    let mut distinct_required_column_names = IndexSet::new();
                    for required_col_name in required_column_names {
                        if distinct_required_column_names.contains(&required_col_name) {
                            all_required_column_names_distinct = false;
                            break;
                        } else {
                            distinct_required_column_names.insert(required_col_name);
                        }
                    }

                    if all_required_column_names_distinct {
                        let mut actor_pool_projection_map = upstream_actor_pool_projection
                            .passthrough_columns
                            .iter()
                            .map(|e| (e.name().to_string(), e.clone()))
                            .collect::<HashMap<String, ExprRef>>();
                        actor_pool_projection_map.insert(
                            upstream_actor_pool_projection.project.name().to_string(),
                            upstream_actor_pool_projection.project.clone(),
                        );

                        let new_actor_pool_projections = projection
                            .projection
                            .iter()
                            .map(|p| {
                                replace_columns_with_expressions(
                                    p.clone(),
                                    &actor_pool_projection_map,
                                )
                            })
                            .collect_vec();

                        // Construct either a new UDFProject or Project, depending on whether the pruned projection still has UDFs
                        let (udf, others): (Vec<_>, Vec<_>) = new_actor_pool_projections
                            .iter()
                            .cloned()
                            .partition(|e| e.exists(is_udf));
                        let new_plan = if !udf.is_empty() {
                            LogicalPlan::UDFProject(UDFProject::try_new(
                                upstream_actor_pool_projection.input.clone(),
                                udf.first().unwrap().clone(),
                                others,
                            )?)
                            .arced()
                        } else {
                            LogicalPlan::Project(Project::try_new(
                                upstream_actor_pool_projection.input.clone(),
                                new_actor_pool_projections,
                            )?)
                            .arced()
                        };

                        // Retry optimization now that the node is different.
                        let new_plan = self
                            .try_optimize_node(new_plan.clone())?
                            .or(Transformed::yes(new_plan));
                        return Ok(new_plan);
                    }
                }

                // Prune columns from the child UDFProjection that are not used in this projection.
                if required_columns.len() < upstream_schema.names().len() {
                    let pruned_upstream_projections = upstream_actor_pool_projection
                        .passthrough_columns
                        .iter()
                        .filter(|&e| required_columns.contains(e.name()))
                        .cloned()
                        .collect::<Vec<_>>();
                    let new_upstream = LogicalPlan::UDFProject(UDFProject::try_new(
                        upstream_actor_pool_projection.input.clone(),
                        upstream_actor_pool_projection.project.clone(),
                        pruned_upstream_projections,
                    )?)
                    .arced();
                    let new_plan = Arc::new(plan.with_new_children(&[new_upstream]));

                    // Retry optimization now that the upstream node is different.
                    let new_plan = self
                        .try_optimize_node(new_plan.clone())?
                        .or(Transformed::yes(new_plan));
                    Ok(new_plan)
                } else {
                    Ok(Transformed::no(plan))
                }
            }
            LogicalPlan::Sort(..)
            | LogicalPlan::Shard(..)
            | LogicalPlan::Repartition(..)
            | LogicalPlan::Limit(..)
            | LogicalPlan::TopN(..)
            | LogicalPlan::Filter(..)
            | LogicalPlan::Sample(..)
            | LogicalPlan::Explode(..) => {
                // Get required columns from projection and upstream.
                let combined_dependencies = plan
                    .required_columns()
                    .iter()
                    .flatten()
                    .chain(upstream_plan.required_columns().iter().flatten())
                    .cloned()
                    .collect::<IndexSet<_>>();

                // Skip optimization if no columns would be pruned.
                let grand_upstream_plan = &upstream_plan.arc_children()[0];
                let grand_upstream_columns = grand_upstream_plan.schema().names();

                if grand_upstream_columns.len() == combined_dependencies.len() {
                    return Ok(Transformed::no(plan));
                }

                let new_subprojection: LogicalPlan = {
                    let pushdown_column_exprs = combined_dependencies
                        .into_iter()
                        .map(resolved_col)
                        .collect::<Vec<_>>();

                    Project::try_new(grand_upstream_plan.clone(), pushdown_column_exprs)?.into()
                };

                let new_upstream = upstream_plan.with_new_children(&[new_subprojection.into()]);
                let new_plan = Arc::new(plan.with_new_children(&[new_upstream.into()]));
                // Retry optimization now that the upstream node is different.
                let new_plan = self
                    .try_optimize_node(new_plan.clone())?
                    .or(Transformed::yes(new_plan));
                Ok(new_plan)
            }
            LogicalPlan::Unpivot(unpivot) => {
                let combined_dependencies = plan
                    .required_columns()
                    .iter()
                    .flatten()
                    .chain(upstream_plan.required_columns().iter().flatten())
                    .cloned()
                    .collect::<IndexSet<_>>();

                let grand_upstream_plan = &upstream_plan.arc_children()[0];
                let grand_upstream_columns = grand_upstream_plan.schema().names();
                let input_columns = unpivot
                    .ids
                    .iter()
                    .chain(unpivot.values.iter())
                    .map(|e| e.name().to_string())
                    .collect::<IndexSet<_>>();

                let can_be_pushed_down = input_columns
                    .intersection(&combined_dependencies)
                    .map(|e| resolved_col(e.as_str()))
                    .collect::<Vec<_>>();

                if grand_upstream_columns.len() == can_be_pushed_down.len() {
                    return Ok(Transformed::no(plan));
                }

                let new_subprojection: LogicalPlan =
                    Project::try_new(grand_upstream_plan.clone(), can_be_pushed_down)?.into();
                let new_upstream = upstream_plan.with_new_children(&[new_subprojection.into()]);
                let new_plan = Arc::new(plan.with_new_children(&[new_upstream.into()]));
                // Retry optimization now that the upstream node is different.
                let new_plan = self
                    .try_optimize_node(new_plan.clone())?
                    .or(Transformed::yes(new_plan));
                Ok(new_plan)
            }
            LogicalPlan::Concat(concat) => {
                // Get required columns from projection and upstream.
                let combined_dependencies = plan
                    .required_columns()
                    .iter()
                    .flatten()
                    .chain(upstream_plan.required_columns().iter().flatten())
                    .cloned()
                    .collect::<IndexSet<_>>();

                // Skip optimization if no columns would be pruned.
                let grand_upstream_plan = &upstream_plan.children()[0];
                let grand_upstream_columns = grand_upstream_plan.schema().names();
                if grand_upstream_columns.len() == combined_dependencies.len() {
                    return Ok(Transformed::no(plan));
                }

                let pushdown_column_exprs: Vec<ExprRef> = combined_dependencies
                    .into_iter()
                    .map(resolved_col)
                    .collect::<Vec<_>>();
                let new_left_subprojection: LogicalPlan = {
                    Project::try_new(concat.input.clone(), pushdown_column_exprs.clone())?.into()
                };
                let new_right_subprojection: LogicalPlan =
                    { Project::try_new(concat.other.clone(), pushdown_column_exprs)?.into() };

                let new_upstream = upstream_plan.with_new_children(&[
                    new_left_subprojection.into(),
                    new_right_subprojection.into(),
                ]);
                let new_plan = Arc::new(plan.with_new_children(&[new_upstream.into()]));
                // Retry optimization now that the upstream node is different.
                let new_plan = self
                    .try_optimize_node(new_plan.clone())?
                    .or(Transformed::yes(new_plan));
                Ok(new_plan)
            }
            LogicalPlan::Union(_) => unreachable!("Union should have been optimized away"),
            LogicalPlan::Join(join) => {
                // Get required columns from projection and both upstreams.
                let [projection_dependencies] = &plan.required_columns()[..] else {
                    panic!()
                };
                let [left_dependencies, right_dependencies] = &upstream_plan.required_columns()[..]
                else {
                    panic!()
                };

                /// For one side of the join, see if a non-vacuous pushdown is possible.
                fn maybe_project_upstream_input(
                    side: &LogicalPlanRef,
                    side_dependencies: &IndexSet<String>,
                    projection_dependencies: &IndexSet<String>,
                ) -> DaftResult<Transformed<LogicalPlanRef>> {
                    let schema = side.schema();
                    let upstream_names: IndexSet<String> =
                        schema.field_names().map(ToString::to_string).collect();

                    let combined_dependencies: IndexSet<_> = side_dependencies
                        .union(
                            &upstream_names
                                .intersection(projection_dependencies)
                                .cloned()
                                .collect::<IndexSet<_>>(),
                        )
                        .cloned()
                        .collect();

                    if combined_dependencies.len() < upstream_names.len() {
                        let pushdown_column_exprs: Vec<ExprRef> = combined_dependencies
                            .into_iter()
                            .map(resolved_col)
                            .collect();
                        let new_project: LogicalPlan =
                            Project::try_new(side.clone(), pushdown_column_exprs)?.into();
                        Ok(Transformed::yes(new_project.into()))
                    } else {
                        Ok(Transformed::no(side.clone()))
                    }
                }

                let new_left_upstream = maybe_project_upstream_input(
                    &join.left,
                    left_dependencies,
                    projection_dependencies,
                )?;
                let new_right_upstream = maybe_project_upstream_input(
                    &join.right,
                    right_dependencies,
                    projection_dependencies,
                )?;

                if !new_left_upstream.transformed && !new_right_upstream.transformed {
                    Ok(Transformed::no(plan))
                } else {
                    // If either pushdown is possible, create a new Join node.
                    let new_join = upstream_plan
                        .with_new_children(&[new_left_upstream.data, new_right_upstream.data]);

                    let new_plan = Arc::new(plan.with_new_children(&[new_join.into()]));

                    // Retry optimization now that the upstream node is different.
                    let new_plan = self
                        .try_optimize_node(new_plan.clone())?
                        .or(Transformed::yes(new_plan));

                    Ok(new_plan)
                }
            }
            LogicalPlan::Distinct(distinct) => {
                if distinct.columns.is_none() {
                    // Cannot push down past a Distinct if the distinct is on all columns
                    return Ok(Transformed::no(plan));
                }

                let plan_req_cols = &plan.required_columns()[0];
                let distinct_req_cols = &upstream_plan.required_columns()[0];

                // Add a new projection underneath the distinct to pass through columns
                // used by the distinct & current projection node
                let new_extra_projection = LogicalPlan::Project(Project::try_new(
                    distinct.input.clone(),
                    plan_req_cols
                        .union(distinct_req_cols)
                        .map(|e| resolved_col(e.as_str()))
                        .collect::<Vec<_>>(),
                )?)
                .arced();

                let new_distinct = upstream_plan
                    .with_new_children(&[new_extra_projection.into()])
                    .arced();
                let new_plan = plan.with_new_children(&[new_distinct]).arced();
                Ok(Transformed::yes(new_plan.into()))
            }
            LogicalPlan::Intersect(_) => {
                // Cannot push down past an Intersect,
                // since Intersect implicitly requires all parent columns.
                Ok(Transformed::no(plan))
            }
            LogicalPlan::Pivot(_) | LogicalPlan::MonotonicallyIncreasingId(_) => {
                // Cannot push down past a Pivot/MonotonicallyIncreasingId because it changes the schema.
                Ok(Transformed::no(plan))
            }
            LogicalPlan::Window(_) => {
                // Cannot push down past a Window because it changes the window calculation results
                Ok(Transformed::no(plan))
            }
            LogicalPlan::Sink(_) => {
                panic!("Bad projection due to upstream sink node: {:?}", projection)
            }
            LogicalPlan::SubqueryAlias(_) => unreachable!("Alias should have been optimized away"),
        }
    }

    fn try_optimize_udf_project(
        &self,
        udf_project: &UDFProject,
        plan: Arc<LogicalPlan>,
    ) -> DaftResult<Transformed<Arc<LogicalPlan>>> {
        // If this UDFProject prunes columns from its upstream,
        // then explicitly create a projection to do so.
        let upstream_plan = &udf_project.input;
        let upstream_schema = upstream_plan.schema();

        let udf_project_required_cols = &plan.required_columns()[0];
        if udf_project_required_cols.len() < upstream_schema.names().len() {
            let new_subprojection: LogicalPlan = {
                let pushdown_column_exprs = udf_project_required_cols
                    .iter()
                    .map(|s| resolved_col(s.as_str()))
                    .collect::<Vec<_>>();

                Project::try_new(upstream_plan.clone(), pushdown_column_exprs)?.into()
            };

            let new_udf_project = plan.with_new_children(&[new_subprojection.into()]);
            Ok(Transformed::yes(new_udf_project.into()))
        } else {
            Ok(Transformed::no(plan))
        }
    }

    fn try_optimize_aggregation(
        &self,
        aggregation: &Aggregate,
        plan: Arc<LogicalPlan>,
    ) -> DaftResult<Transformed<Arc<LogicalPlan>>> {
        // If this aggregation prunes columns from its upstream,
        // then explicitly create a projection to do so.
        let upstream_plan = &aggregation.input;
        let upstream_schema = upstream_plan.schema();

        let aggregation_required_cols = &plan.required_columns()[0];
        if aggregation_required_cols.len() < upstream_schema.names().len() {
            let new_subprojection: LogicalPlan = {
                let pushdown_column_exprs = aggregation_required_cols
                    .iter()
                    .map(|s| resolved_col(s.as_str()))
                    .collect::<Vec<_>>();

                Project::try_new(upstream_plan.clone(), pushdown_column_exprs)?.into()
            };

            let new_aggregation = plan.with_new_children(&[new_subprojection.into()]);
            Ok(Transformed::yes(new_aggregation.into()))
        } else {
            Ok(Transformed::no(plan))
        }
    }

    fn try_optimize_join(
        &self,
        join: &Join,
        plan: Arc<LogicalPlan>,
    ) -> DaftResult<Transformed<Arc<LogicalPlan>>> {
        // If this join prunes columns from its upstream,
        // then explicitly create a projection to do so.
        // this is the case for semi and anti joins.

        if matches!(join.join_type, JoinType::Anti | JoinType::Semi) {
            let required_cols = plan.required_columns();
            let right_required_cols = required_cols
                .get(1)
                .expect("we expect 2 set of required columns for join");
            let right_schema = join.right.schema();

            if right_required_cols.len() < right_schema.len() {
                let new_subprojection: LogicalPlan = {
                    let pushdown_column_exprs = right_required_cols
                        .iter()
                        .map(|s| resolved_col(s.as_str()))
                        .collect::<Vec<_>>();

                    Project::try_new(join.right.clone(), pushdown_column_exprs)?.into()
                };

                let new_join = plan
                    .with_new_children(&[(join.left).clone(), new_subprojection.into()])
                    .arced();

                Ok(self
                    .try_optimize_node(new_join.clone())?
                    .or(Transformed::yes(new_join)))
            } else {
                Ok(Transformed::no(plan))
            }
        } else {
            Ok(Transformed::no(plan))
        }
    }

    fn try_optimize_pivot(
        &self,
        pivot: &Pivot,
        plan: Arc<LogicalPlan>,
    ) -> DaftResult<Transformed<Arc<LogicalPlan>>> {
        // If this pivot prunes columns from its upstream,
        // then explicitly create a projection to do so.
        let upstream_plan = &pivot.input;
        let upstream_schema = upstream_plan.schema();

        let pivot_required_cols = &plan.required_columns()[0];
        if pivot_required_cols.len() < upstream_schema.names().len() {
            let new_subprojection: LogicalPlan = {
                let pushdown_column_exprs = pivot_required_cols
                    .iter()
                    .map(|s| resolved_col(s.as_str()))
                    .collect::<Vec<_>>();

                Project::try_new(upstream_plan.clone(), pushdown_column_exprs)?.into()
            };

            let new_pivot = plan.with_new_children(&[new_subprojection.into()]);
            Ok(Transformed::yes(new_pivot.into()))
        } else {
            Ok(Transformed::no(plan))
        }
    }

    fn try_optimize_node(
        &self,
        plan: Arc<LogicalPlan>,
    ) -> DaftResult<Transformed<Arc<LogicalPlan>>> {
        match plan.as_ref() {
            LogicalPlan::Project(projection) => self.try_optimize_project(projection, plan.clone()),
            // UDFProjects also do column pruning
            LogicalPlan::UDFProject(udf_project) => {
                self.try_optimize_udf_project(udf_project, plan.clone())
            }
            // Aggregations also do column projection
            LogicalPlan::Aggregate(aggregation) => {
                self.try_optimize_aggregation(aggregation, plan.clone())
            }
            // Joins also do column projection
            LogicalPlan::Join(join) => self.try_optimize_join(join, plan.clone()),
            // Pivots also do column projection
            LogicalPlan::Pivot(pivot) => self.try_optimize_pivot(pivot, plan.clone()),
            _ => Ok(Transformed::no(plan)),
        }
    }
}

impl OptimizerRule for PushDownProjection {
    fn try_optimize(&self, plan: Arc<LogicalPlan>) -> DaftResult<Transformed<Arc<LogicalPlan>>> {
        let out = plan.transform_down(|node| self.try_optimize_node(node))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use common_error::DaftResult;
    use common_resource_request::ResourceRequest;
    use common_scan_info::Pushdowns;
    use daft_core::prelude::*;
    use daft_dsl::{
        functions::{
            python::{MaybeInitializedUDF, PythonUDF, RuntimePyObject},
            FunctionExpr,
        },
        lit, resolved_col, unresolved_col, Expr, ExprRef,
    };

    use crate::{
        ops::{Project, Unpivot},
        optimization::{
            optimizer::{RuleBatch, RuleExecutionStrategy},
            rules::PushDownProjection,
            test::assert_optimized_plan_with_rules_eq,
        },
        test::{dummy_scan_node, dummy_scan_node_with_pushdowns, dummy_scan_operator},
        LogicalPlan,
    };

    /// Helper that creates an optimizer with the PushDownProjection rule registered, optimizes
    /// the provided plan with said optimizer, and compares the optimized plan with
    /// the provided expected plan.
    fn assert_optimized_plan_eq(
        plan: Arc<LogicalPlan>,
        expected: Arc<LogicalPlan>,
    ) -> DaftResult<()> {
        assert_optimized_plan_with_rules_eq(
            plan,
            expected,
            vec![RuleBatch::new(
                vec![Box::new(PushDownProjection::new())],
                RuleExecutionStrategy::Once,
            )],
        )
    }

    fn create_actor_pool_udf(inputs: Vec<ExprRef>) -> ExprRef {
        Expr::Function {
            func: FunctionExpr::Python(PythonUDF {
                name: Arc::new("my-udf".to_string()),
                func: MaybeInitializedUDF::Uninitialized {
                    inner: RuntimePyObject::new_none(),
                    init_args: RuntimePyObject::new_none(),
                },
                bound_args: RuntimePyObject::new_none(),
                num_expressions: inputs.len(),
                return_dtype: DataType::Utf8,
                resource_request: Some(ResourceRequest::default_cpu()),
                batch_size: None,
                concurrency: Some(8),
                use_process: None,
            }),
            inputs,
        }
        .arced()
    }

    /// Projection merging: Ensure factored projections do not get merged.
    #[test]
    fn test_merge_does_not_unfactor() -> DaftResult<()> {
        let a2 = unresolved_col("a").add(unresolved_col("a"));
        let a4 = a2.clone().add(a2);
        let a8 = a4.clone().add(a4);
        let expressions = vec![a8.alias("x")];
        let scan_op = dummy_scan_operator(vec![Field::new("a", DataType::Int64)]);
        let plan = dummy_scan_node(scan_op).select(expressions)?.build();

        assert_optimized_plan_eq(plan.clone(), plan)?;
        Ok(())
    }

    /// Projection merging: Ensure merging happens even when there is computation
    /// in both the parent and the child.
    #[test]
    fn test_merge_projections() -> DaftResult<()> {
        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Int64),
        ]);
        let proj1 = vec![
            unresolved_col("a").add(lit(1)),
            unresolved_col("b").add(lit(2)),
            unresolved_col("a").alias("c"),
        ];
        let proj2 = vec![
            unresolved_col("a").add(lit(3)),
            unresolved_col("b"),
            unresolved_col("c").add(lit(4)),
        ];
        let plan = dummy_scan_node(scan_op.clone())
            .select(proj1)?
            .select(proj2)?
            .build();

        let merged_proj = vec![
            unresolved_col("a").add(lit(1)).add(lit(3)),
            unresolved_col("b").add(lit(2)),
            unresolved_col("a").alias("c").add(lit(4)),
        ];
        let expected = dummy_scan_node(scan_op).select(merged_proj)?.build();

        assert_optimized_plan_eq(plan, expected)?;
        Ok(())
    }

    /// Projection dropping: Test that a no-op projection is dropped.
    #[test]
    fn test_drop_projection() -> DaftResult<()> {
        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Int64),
        ]);
        let plan = dummy_scan_node(scan_op.clone())
            .select(vec![unresolved_col("a"), unresolved_col("b")])?
            .build();

        let expected = dummy_scan_node(scan_op).build();

        assert_optimized_plan_eq(plan, expected)?;

        Ok(())
    }

    /// Projection dropping: Test that projections doing reordering are not dropped.
    #[test]
    fn test_dont_drop_projection() -> DaftResult<()> {
        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Int64),
        ]);
        let proj = vec![unresolved_col("b"), unresolved_col("a")];
        let plan = dummy_scan_node(scan_op.clone())
            .select(proj.clone())?
            .build();

        let expected = dummy_scan_node(scan_op).select(proj)?.build();

        assert_optimized_plan_eq(plan, expected)?;

        Ok(())
    }

    /// Projection<-Source
    #[test]
    fn test_projection_source() -> DaftResult<()> {
        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Int64),
        ]);
        let proj = vec![unresolved_col("b").add(lit(3))];
        let plan = dummy_scan_node(scan_op.clone())
            .select(proj.clone())?
            .build();

        let proj_pushdown = vec!["b".to_string()];
        let expected = dummy_scan_node_with_pushdowns(
            scan_op,
            Pushdowns::default().with_columns(Some(Arc::new(proj_pushdown))),
        )
        .select(proj)?
        .build();

        assert_optimized_plan_eq(plan, expected)?;

        Ok(())
    }

    /// Projection<-Projection column pruning
    #[test]
    fn test_projection_projection() -> DaftResult<()> {
        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Int64),
        ]);
        let proj1 = vec![
            unresolved_col("b").add(lit(3)),
            unresolved_col("a"),
            unresolved_col("a").alias("x"),
        ];
        let proj2 = vec![
            unresolved_col("a"),
            unresolved_col("b"),
            unresolved_col("b").alias("c"),
        ];
        let plan = dummy_scan_node(scan_op.clone())
            .select(proj1)?
            .select(proj2.clone())?
            .build();

        let new_proj1 = vec![unresolved_col("b").add(lit(3)), unresolved_col("a")];
        let expected = dummy_scan_node(scan_op)
            .select(new_proj1)?
            .select(proj2)?
            .build();

        assert_optimized_plan_eq(plan, expected)?;

        Ok(())
    }

    /// Projection<-Aggregation column pruning
    #[test]
    fn test_projection_aggregation() -> DaftResult<()> {
        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Int64),
            Field::new("c", DataType::Int64),
        ]);
        let agg = vec![unresolved_col("a").mean(), unresolved_col("b").mean()];
        let group_by = vec![unresolved_col("c")];
        let proj = vec![unresolved_col("a")];
        let plan = dummy_scan_node(scan_op.clone())
            .aggregate(agg, group_by.clone())?
            .select(proj.clone())?
            .build();

        let proj_pushdown = vec!["a".to_string(), "c".to_string()];
        let new_agg = vec![unresolved_col("a").mean()];
        let expected = dummy_scan_node_with_pushdowns(
            scan_op,
            Pushdowns::default().with_columns(Some(Arc::new(proj_pushdown))),
        )
        .aggregate(new_agg, group_by)?
        .select(proj)?
        .build();

        assert_optimized_plan_eq(plan, expected)?;

        Ok(())
    }

    /// Projection<-X pushes down the combined required columns
    #[test]
    fn test_projection_pushdown() -> DaftResult<()> {
        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Boolean),
            Field::new("c", DataType::Int64),
        ]);
        let pred = unresolved_col("b");
        let proj = vec![unresolved_col("a")];
        let plan = dummy_scan_node(scan_op.clone())
            .filter(pred.clone())?
            .select(proj.clone())?
            .build();

        let proj_pushdown = vec!["a".to_string(), "b".to_string()];
        let expected = dummy_scan_node_with_pushdowns(
            scan_op,
            Pushdowns::default().with_columns(Some(Arc::new(proj_pushdown))),
        )
        .filter(pred)?
        .select(proj)?
        .build();

        assert_optimized_plan_eq(plan, expected)?;

        Ok(())
    }

    /// Projection does not push down past monotonically increasing id
    #[test]
    fn test_projection_no_pushdown_monotonically_increasing_id() -> DaftResult<()> {
        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Int64),
        ]);
        let plan = dummy_scan_node(scan_op.clone())
            .add_monotonically_increasing_id(Some("id"), None)?
            .select(vec![unresolved_col("id")])?
            .build();
        let expected = plan.clone();
        assert_optimized_plan_eq(plan, expected)?;

        Ok(())
    }

    /// Projection<-UDFProject prunes columns from the UDFProject
    #[test]
    fn test_projection_pushdown_into_udf_project() -> DaftResult<()> {
        use crate::ops::{Project, UDFProject};

        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Boolean),
            Field::new("c", DataType::Int64),
        ]);
        let scan_node = dummy_scan_node(scan_op.clone());
        let mock_udf = create_actor_pool_udf(vec![resolved_col("c")]);

        // Select the `udf_results` column, so the UDFProject should apply column pruning to the other columns
        let udf_project = LogicalPlan::UDFProject(UDFProject::try_new(
            scan_node.build(),
            mock_udf.alias("udf_results"),
            vec![resolved_col("a"), resolved_col("b")],
        )?)
        .arced();
        let project = LogicalPlan::Project(Project::try_new(
            udf_project,
            vec![resolved_col("udf_results")],
        )?)
        .arced();

        let expected_udf_project = LogicalPlan::UDFProject(UDFProject::try_new(
            dummy_scan_node_with_pushdowns(
                scan_op,
                Pushdowns::default().with_columns(Some(Arc::new(vec!["c".to_string()]))),
            )
            .build(),
            mock_udf.alias("udf_results"),
            vec![],
        )?)
        .arced();

        assert_optimized_plan_eq(project, expected_udf_project)?;
        Ok(())
    }

    /// Projection<-UDFProject<-UDFProject prunes columns from both UDFProjects
    #[test]
    fn test_projection_pushdown_into_double_udf_project() -> DaftResult<()> {
        use crate::ops::{Project, UDFProject};

        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Boolean),
            Field::new("c", DataType::Int64),
        ]);
        let scan_node = dummy_scan_node(scan_op.clone()).build();
        let mock_udf = create_actor_pool_udf(vec![resolved_col("a")]);

        // Select the `udf_results` column, so the UDFProject should apply column pruning to the other columns
        let plan = LogicalPlan::UDFProject(UDFProject::try_new(
            scan_node,
            mock_udf.alias("udf_results_0"),
            vec![resolved_col("a"), resolved_col("b")],
        )?)
        .arced();

        let plan = LogicalPlan::UDFProject(UDFProject::try_new(
            plan,
            mock_udf.alias("udf_results_1"),
            vec![
                resolved_col("a"),
                resolved_col("b"),
                resolved_col("udf_results_0"),
            ],
        )?)
        .arced();

        let plan = LogicalPlan::Project(Project::try_new(
            plan,
            vec![
                resolved_col("udf_results_0").alias("udf_results_0_alias"),
                resolved_col("udf_results_1"),
            ],
        )?)
        .arced();

        let expected = LogicalPlan::UDFProject(UDFProject::try_new(
            dummy_scan_node_with_pushdowns(
                scan_op,
                Pushdowns::default().with_columns(Some(Arc::new(vec!["a".to_string()]))),
            )
            .build(),
            // col("b") is pruned
            mock_udf.alias("udf_results_0"),
            vec![resolved_col("a")],
        )?)
        .arced();
        let expected = LogicalPlan::UDFProject(UDFProject::try_new(
            expected,
            mock_udf.alias("udf_results_1"),
            vec![
                // Absorbed a non-computational expression (alias) from the Projection
                resolved_col("udf_results_0").alias("udf_results_0_alias"),
            ],
        )?)
        .arced();

        assert_optimized_plan_eq(plan, expected)?;
        Ok(())
    }

    /// Projection<-UDFProject prunes UDFProject entirely if the UDF column is pruned
    #[test]
    fn test_projection_pushdown_into_udf_project_completely_removed() -> DaftResult<()> {
        use crate::ops::{Project, UDFProject};

        let scan_op = dummy_scan_operator(vec![
            Field::new("a", DataType::Int64),
            Field::new("b", DataType::Boolean),
            Field::new("c", DataType::Int64),
        ]);
        let scan_node = dummy_scan_node(scan_op.clone()).build();
        let mock_udf = create_actor_pool_udf(vec![resolved_col("c")]);

        // Select only col("a"), so the UDFProject node is now redundant and should be removed
        let udf_project = LogicalPlan::UDFProject(UDFProject::try_new(
            scan_node,
            mock_udf.alias("udf_results"),
            vec![resolved_col("a"), resolved_col("b")],
        )?)
        .arced();
        let project =
            LogicalPlan::Project(Project::try_new(udf_project, vec![resolved_col("a")])?).arced();

        // Optimized plan will push the projection all the way down into the scan
        let expected_scan = dummy_scan_node_with_pushdowns(
            scan_op.clone(),
            Pushdowns {
                limit: None,
                partition_filters: None,
                columns: Some(Arc::new(vec!["a".to_string()])),
                filters: None,
                sharder: None,
            },
        )
        .build();

        assert_optimized_plan_eq(project, expected_scan)?;
        Ok(())
    }

    #[test]
    fn test_projection_pushdown_with_unpivot() {
        let scan_op = dummy_scan_operator(vec![
            Field::new("year", DataType::Int64),
            Field::new("id", DataType::Int64),
            Field::new("Jan", DataType::Int64),
            Field::new("Feb", DataType::Int64),
        ]);
        let scan_node = dummy_scan_node(scan_op.clone()).build();

        let plan = LogicalPlan::Unpivot(
            Unpivot::try_new(
                scan_node.clone(),
                vec![resolved_col("year")],
                vec![resolved_col("Jan"), resolved_col("Feb")],
                "month".to_string(),
                "inventory".to_string(),
            )
            .unwrap(),
        );

        let plan = LogicalPlan::Project(
            Project::try_new(plan.into(), vec![resolved_col("inventory").alias("year2")]).unwrap(),
        )
        .into();
        let expected_scan = dummy_scan_node_with_pushdowns(
            scan_op.clone(),
            Pushdowns {
                limit: None,
                partition_filters: None,
                columns: Some(Arc::new(vec![
                    "year".to_string(),
                    "Jan".to_string(),
                    "Feb".to_string(),
                ])),
                filters: None,
                sharder: None,
            },
        )
        .build();

        let expected = LogicalPlan::Unpivot(
            Unpivot::try_new(
                expected_scan,
                vec![resolved_col("year")],
                vec![resolved_col("Jan"), resolved_col("Feb")],
                "month".to_string(),
                "inventory".to_string(),
            )
            .unwrap(),
        );

        let expected = LogicalPlan::Project(
            Project::try_new(
                expected.into(),
                vec![resolved_col("inventory").alias("year2")],
            )
            .unwrap(),
        )
        .into();
        assert_optimized_plan_eq(plan, expected).unwrap();
    }
}
