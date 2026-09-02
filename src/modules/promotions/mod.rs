// Promotions module — READ-ONLY, engine-adaptive, over the local DB.
// Both engines materialize into promo_rules (kind 'special' | 'rbp_condition');
// RBP set/group chain materializes into pricing_sets / pricing_groups
// (migration 0004) so PROSET resolution is fully local.
// rbp.rs: resolver (classify ported verbatim + local resolution).
// db.rs: list / items / effectiveness (clip_range/base_window/promo_math/
//        fifo_blend_avg_cost ported verbatim from WebRMS R13).
pub mod db;
pub mod handlers;
pub mod rbp;
