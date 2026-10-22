//! Interactive Dashboard with `mview!` - showcasing all syntax sugar

use veloasm::{
    signal, signal_vec, mview, DomNode, RwSignal,
    interval, RwSignal as Signal,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Mock Data Models
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Metric {
    id: String,
    label: String,
    value: f64,
    change: f64,
    trend: Trend,
    icon: &'static str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Trend { Up, Down, Neutral }

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Order {
    id: String,
    customer: String,
    email: String,
    amount: f64,
    status: OrderStatus,
    date: String,
    items: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum OrderStatus { Pending, Processing, Shipped, Delivered, Cancelled }

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChartDataPoint {
    label: String,
    value: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Activity {
    id: String,
    user: String,
    action: String,
    time: String,
    icon: &'static str,
}

// ============================================================================
// Mock Data Generators
// ============================================================================

fn mock_metrics() -> Vec<Metric> {
    vec![
        Metric { id: "revenue".into(), label: "Total Revenue".into(), value: 128450.00, change: 12.5, trend: Trend::Up, icon: "💰" },
        Metric { id: "orders".into(), label: "Total Orders".into(), value: 1234.0, change: 8.2, trend: Trend::Up, icon: "📦" },
        Metric { id: "customers".into(), label: "Active Customers".into(), value: 567.0, change: -2.1, trend: Trend::Down, icon: "👥" },
        Metric { id: "conversion".into(), label: "Conversion Rate".into(), value: 3.24, change: 0.5, trend: Trend::Up, icon: "📈" },
    ]
}

fn mock_orders() -> Vec<Order> {
    vec![
        Order { id: "ORD-001".into(), customer: "Acme Corp".into(), email: "orders@acme.com".into(), amount: 2450.00, status: OrderStatus::Delivered, date: "2024-01-15".into(), items: 12 },
        Order { id: "ORD-002".into(), customer: "Globex Inc".into(), email: "procurement@globex.com".into(), amount: 890.50, status: OrderStatus::Shipped, date: "2024-01-14".into(), items: 5 },
        Order { id: "ORD-003".into(), customer: "Wayne Enterprises".into(), email: "supply@wayne.com".into(), amount: 15600.00, status: OrderStatus::Processing, date: "2024-01-13".into(), items: 28 },
        Order { id: "ORD-004".into(), customer: "Stark Industries".into(), email: "logistics@stark.com".into(), amount: 3200.75, status: OrderStatus::Pending, date: "2024-01-12".into(), items: 8 },
        Order { id: "ORD-005".into(), customer: "Umbrella Corp".into(), email: "ops@umbrella.com".into(), amount: 540.00, status: OrderStatus::Cancelled, date: "2024-01-11".into(), items: 3 },
        Order { id: "ORD-006".into(), customer: "Cyberdyne Systems".into(), email: "procure@cyberdyne.com".into(), amount: 8750.25, status: OrderStatus::Delivered, date: "2024-01-10".into(), items: 15 },
        Order { id: "ORD-007".into(), customer: "Massive Dynamic".into(), email: "supply@massivedynamic.com".into(), amount: 12300.00, status: OrderStatus::Shipped, date: "2024-01-09".into(), items: 22 },
        Order { id: "ORD-008".into(), customer: "Initech".into(), email: "orders@initech.com".into(), amount: 680.00, status: OrderStatus::Pending, date: "2024-01-08".into(), items: 4 },
    ]
}

fn mock_revenue_chart() -> Vec<ChartDataPoint> {
    vec![
        ChartDataPoint { label: "Mon".into(), value: 12000.0 },
        ChartDataPoint { label: "Tue".into(), value: 19000.0 },
        ChartDataPoint { label: "Wed".into(), value: 15000.0 },
        ChartDataPoint { label: "Thu".into(), value: 22000.0 },
        ChartDataPoint { label: "Fri".into(), value: 18000.0 },
        ChartDataPoint { label: "Sat".into(), value: 25000.0 },
        ChartDataPoint { label: "Sun".into(), value: 16000.0 },
    ]
}

fn mock_orders_chart() -> Vec<ChartDataPoint> {
    vec![
        ChartDataPoint { label: "Week 1".into(), value: 45.0 },
        ChartDataPoint { label: "Week 2".into(), value: 52.0 },
        ChartDataPoint { label: "Week 3".into(), value: 38.0 },
        ChartDataPoint { label: "Week 4".into(), value: 61.0 },
        ChartDataPoint { label: "Week 5".into(), value: 55.0 },
        ChartDataPoint { label: "Week 6".into(), value: 67.0 },
    ]
}

fn mock_activities() -> Vec<Activity> {
    vec![
        Activity { id: "1".into(), user: "Sarah Chen".into(), action: "placed order ORD-009".into(), time: "2 min ago".into(), icon: "🛒" },
        Activity { id: "2".into(), user: "Mike Johnson".into(), action: "updated profile".into(), time: "15 min ago".into(), icon: "👤" },
        Activity { id: "3".into(), user: "Emily Davis".into(), action: "left review ⭐⭐⭐⭐⭐".into(), time: "1 hour ago".into(), icon: "⭐" },
        Activity { id: "4".into(), user: "James Wilson".into(), action: "cancelled order ORD-005".into(), time: "3 hours ago".into(), icon: "❌" },
        Activity { id: "5".into(), user: "Lisa Anderson".into(), action: "referred new customer".into(), time: "5 hours ago".into(), icon: "🔗" },
        Activity { id: "6".into(), user: "David Brown".into(), action: "upgraded subscription".into(), time: "1 day ago".into(), icon: "⬆️" },
    ]
}

// ============================================================================
// Dashboard State
// ============================================================================

struct DashboardState {
    metrics: RwSignal<Vec<Metric>>,
    orders: RwSignal<Vec<Order>>,
    revenue_chart: RwSignal<Vec<ChartDataPoint>>,
    orders_chart: RwSignal<Vec<ChartDataPoint>>,
    activities: RwSignal<Vec<Activity>>,
    search_query: RwSignal<String>,
    status_filter: RwSignal<Option<OrderStatus>>,
    sort_column: RwSignal<String>,
    sort_ascending: RwSignal<bool>,
    active_tab: RwSignal<Tab>,
    sidebar_open: RwSignal<bool>,
    theme_dark: RwSignal<bool>,
    notifications: RwSignal<u32>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab { Overview, Orders, Analytics, Activity }

impl DashboardState {
    fn new() -> Self {
        Self {
            metrics: signal(mock_metrics()),
            orders: signal(mock_orders()),
            revenue_chart: signal(mock_revenue_chart()),
            orders_chart: signal(mock_orders_chart()),
            activities: signal(mock_activities()),
            search_query: signal(String::new()),
            status_filter: signal(None),
            sort_column: signal("date".into()),
            sort_ascending: signal(false),
            active_tab: signal(Tab::Overview),
            sidebar_open: signal(true),
            theme_dark: signal(false),
            notifications: signal(3),
        }
    }

    fn filtered_orders(&self) -> Vec<Order> {
        let query = self.search_query.get().to_lowercase();
        let filter = self.status_filter.get();
        let sort_col = self.sort_column.get();
        let ascending = self.sort_ascending.get();

        let mut result: Vec<Order> = self.orders.get()
            .into_iter()
            .filter(|o| {
                let matches_search = query.is_empty() 
                    || o.id.to_lowercase().contains(&query)
                    || o.customer.to_lowercase().contains(&query)
                    || o.email.to_lowercase().contains(&query);
                let matches_filter = filter.as_ref().map(|f| o.status == *f).unwrap_or(true);
                matches_search && matches_filter
            })
            .collect();

        result.sort_by(|a, b| {
            let cmp = match sort_col.as_str() {
                "id" => a.id.cmp(&b.id),
                "customer" => a.customer.cmp(&b.customer),
                "amount" => a.amount.partial_cmp(&b.amount).unwrap(),
                "status" => format!("{:?}", a.status).cmp(&format!("{:?}", b.status)),
                "date" => a.date.cmp(&b.date),
                _ => a.date.cmp(&b.date),
            };
            if ascending { cmp } else { cmp.reverse() }
        });

        result
    }
}

// ============================================================================
// Components
// ============================================================================

#[veloasm::component]
fn MetricCard(metric: Metric) -> DomNode {
    let trend_class = match metric.trend {
        Trend::Up => "trend-up",
        Trend::Down => "trend-down",
        Trend::Neutral => "trend-neutral",
    };
    let change_sign = if metric.change >= 0.0 { "+" } else { "" };

    mview! {
        div class="metric-card" {
            div class="metric-header" {
                span class="metric-icon" { metric.icon.clone() }
                span class="metric-label" { metric.label.clone() }
            }
            div class="metric-value" { format!("{:.2}", metric.value) }
            div class="metric-change" class:trend-up={ metric.trend == Trend::Up } class:trend-down={ metric.trend == Trend::Down } {
                span { format!("{} {:.1}%", change_sign, metric.change) }
                @if metric.trend == Trend::Up { "↑" } @else if metric.trend == Trend::Down { "↓" } @else { "→" }
            }
        }
    }
}

#[veloasm::component]
fn StatusBadge(status: OrderStatus) -> DomNode {
    let (class, label) = match status {
        OrderStatus::Pending => ("badge-pending", "Pending"),
        OrderStatus::Processing => ("badge-processing", "Processing"),
        OrderStatus::Shipped => ("badge-shipped", "Shipped"),
        OrderStatus::Delivered => ("badge-delivered", "Delivered"),
        OrderStatus::Cancelled => ("badge-cancelled", "Cancelled"),
    };
    mview! {
        span class="badge" class={ class } { label }
    }
}

#[veloasm::component]
fn ChartBar(label: String, value: f64, max: f64, color: String) -> DomNode {
    let height_pct = (value / max * 100.0).min(100.0);
    mview! {
        div class="chart-bar" {
            div class="bar-fill" style:height={ format!("{}%", height_pct) } style:background={ color } title={ format!("{}: {}", label, value) } {}
            span class="bar-label" { label }
            span class="bar-value" { format!("{:.0f}", value) }
        }
    }
}

#[veloasm::component]
fn ActivityItem(activity: Activity) -> DomNode {
    mview! {
        div class="activity-item" {
            span class="activity-icon" { activity.icon }
            div class="activity-content" {
                div class="activity-user" { activity.user }
                div class="activity-action" { activity.action }
                span class="activity-time" { activity.time }
            }
        }
    }
}

#[veloasm::component]
fn DataTable(state: DashboardState) -> DomNode {
    let orders = state.filtered_orders();

    mview! {
        div class="data-table-container" {
            table class="data-table" {
                thead {
                    tr {
                        @for col in ["ID", "Customer", "Email", "Amount", "Status", "Date", "Items"] {
                            th @click={ 
                                let col = col.to_string();
                                move || { state.sort_column.set(col.clone()); state.sort_ascending.set(!state.sort_ascending.get()) } 
                            } {
                                { col }
                                @if state.sort_column.get() == col {
                                    @if state.sort_ascending.get() { " ↑" } @else { " ↓" }
                                }
                            }
                        }
                    }
                }
                tbody {
                    @for order in orders {
                        tr {
                            td { order.id }
                            td { order.customer }
                            td { order.email }
                            td { format!("${:.2}", order.amount) }
                            td { StatusBadge { status: order.status.clone() } }
                            td { order.date }
                            td { order.items }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// Main Dashboard View
// ============================================================================

fn dashboard() -> DomNode {
    let state = DashboardState::new();

    // Simulate real-time updates
    let metrics_clone = state.metrics.clone();
    interval(30000, move || {
        let mut metrics = metrics_clone.get();
        for m in &mut metrics {
            m.value += (rand::random::<f64>() - 0.5) * 100.0;
            m.change += (rand::random::<f64>() - 0.5) * 2.0;
            m.trend = if m.change > 0.0 { Trend::Up } else if m.change < 0.0 { Trend::Down } else { Trend::Neutral };
        }
        metrics_clone.set(metrics);
    });

    let activities_clone = state.activities.clone();
    let notifications_clone = state.notifications.clone();
    interval(60000, move || {
        let mut acts = activities_clone.get();
        let new_act = Activity {
            id: format!("{}", acts.len() + 1),
            user: "System".into(),
            action: "automated sync completed".into(),
            time: "just now".into(),
            icon: "🔄",
        };
        acts.insert(0, new_act);
        if acts.len() > 10 { acts.pop(); }
        activities_clone.set(acts);
        notifications_clone.set(notifications_clone.get() + 1);
    });

    mview! {
        div class="dashboard" class:dark={ state.theme_dark.get() } {
            // Header
            header class="dashboard-header" {
                div class="header-left" {
                    button class="hamburger" @click={ move || state.sidebar_open.set(!state.sidebar_open.get()) } { "☰" }
                    h1 class="logo" { "Dashboard" }
                }
                div class="header-center" {
                    input class="search-input" type="search" placeholder="Search orders, customers..." 
                        bind:value={ state.search_query }
                    {}
                }
                div class="header-right" {
                    button class="theme-toggle" @click={ move || state.theme_dark.set(!state.theme_dark.get()) } 
                        class:active={ state.theme_dark.get() }
                        title={ @if state.theme_dark.get() { "Switch to light mode" } @else { "Switch to dark mode" } }
                    {
                        @if state.theme_dark.get() { "☀️" } @else { "🌙" }
                    }
                    div class="notification-bell" class:has-notifications={ state.notifications.get() > 0 } @click={ move || state.notifications.set(0) } {
                        "🔔"
                        @if state.notifications.get() > 0 {
                            span class="badge-count" { state.notifications.get() }
                        }
                    }
                    div class="user-avatar" { "JD" }
                }
            }

            // Sidebar
            aside class="sidebar" class:open={ state.sidebar_open.get() } class:closed={ !state.sidebar_open.get() } {
                nav class="sidebar-nav" {
                    div class="sidebar-section" {
                        h3 { "Dashboard" }
                        button class="tab-button" class:active={ state.active_tab.get() == Tab::Overview } @click={ move || state.active_tab.set(Tab::Overview) } { "📊 Overview" }
                        button class="tab-button" class:active={ state.active_tab.get() == Tab::Orders } @click={ move || state.active_tab.set(Tab::Orders) } { "📦 Orders" }
                        button class="tab-button" class:active={ state.active_tab.get() == Tab::Analytics } @click={ move || state.active_tab.set(Tab::Analytics) } { "📈 Analytics" }
                        button class="tab-button" class:active={ state.active_tab.get() == Tab::Activity } @click={ move || state.active_tab.set(Tab::Activity) } { "🔔 Activity" }
                    }
                    div class="sidebar-section" {
                        h3 { "Filters" }
                        select class="status-filter" bind:value={ state.status_filter } {
                            option value="" { "All Statuses" }
                            option value="Pending" { "Pending" }
                            option value="Processing" { "Processing" }
                            option value="Shipped" { "Shipped" }
                            option value="Delivered" { "Delivered" }
                            option value="Cancelled" { "Cancelled" }
                        }
                    }
                    div class="sidebar-section" {
                        h3 { "Quick Actions" }
                        button class="action-btn" @click={ move || { state.orders.set(mock_orders()); state.metrics.set(mock_metrics()); } } { "🔄 Refresh Data" }
                        button class="action-btn" @click={ move || { /* export logic */ } } { "📥 Export CSV" }
                        button class="action-btn" @click={ move || { state.theme_dark.set(!state.theme_dark.get()) } } { "🎨 Toggle Theme" }
                    }
                }
            }

            // Main Content
            main class="dashboard-main" {
                @if state.active_tab.get() == Tab::Overview {
                    div class="tab-content" {
                        // Metrics Grid
                        section class="metrics-grid" {
                            @for metric in state.metrics.get() {
                                MetricCard { metric: metric.clone() }
                            }
                        }

                        // Charts Row
                        div class="charts-row" {
                            div class="chart-card" {
                                h3 { "Revenue (Last 7 Days)" }
                                div class="chart-bars" {
                                    let rev_data = state.revenue_chart.get();
                                    let max_rev = rev_data.iter().map(|d| d.value).fold(0.0, f64::max);
                                    @for point in rev_data {
                                        ChartBar { 
                                            label: point.label.clone(), 
                                            value: point.value, 
                                            max: max_rev, 
                                            color: "#3b82f6".into() 
                                        }
                                    }
                                }
                            }
                            div class="chart-card" {
                                h3 { "Orders (Last 6 Weeks)" }
                                div class="chart-bars" {
                                    let ord_data = state.orders_chart.get();
                                    let max_ord = ord_data.iter().map(|d| d.value).fold(0.0, f64::max);
                                    @for point in ord_data {
                                        ChartBar { 
                                            label: point.label.clone(), 
                                            value: point.value, 
                                            max: max_ord, 
                                            color: "#10b981".into() 
                                        }
                                    }
                                }
                            }
                        }

                        // Recent Activity
                        div class="activity-feed" {
                            h3 { "Recent Activity" }
                            @for activity in state.activities.get() {
                                ActivityItem { activity: activity.clone() }
                            }
                        }
                    }
                }

                @if state.active_tab.get() == Tab::Orders {
                    div class="tab-content" {
                        div class="orders-header" {
                            h2 { "Orders" }
                            span class="count" { format!("{} orders", state.filtered_orders().len()) }
                        }
                        DataTable { state: state.clone() }
                    }
                }

                @if state.active_tab.get() == Tab::Analytics {
                    div class="tab-content" {
                        h2 { "Analytics Deep Dive" }
                        div class="analytics-grid" {
                            div class="analytics-card" {
                                h3 { "Revenue Trend" }
                                div class="chart-bars large" {
                                    let rev_data = state.revenue_chart.get();
                                    let max_rev = rev_data.iter().map(|d| d.value).fold(0.0, f64::max);
                                    @for point in rev_data {
                                        ChartBar { 
                                            label: point.label.clone(), 
                                            value: point.value, 
                                            max: max_rev, 
                                            color: "#3b82f6".into() 
                                        }
                                    }
                                }
                            }
                            div class="analytics-card" {
                                h3 { "Order Volume" }
                                div class="chart-bars large" {
                                    let ord_data = state.orders_chart.get();
                                    let max_ord = ord_data.iter().map(|d| d.value).fold(0.0, f64::max);
                                    @for point in ord_data {
                                        ChartBar { 
                                            label: point.label.clone(), 
                                            value: point.value, 
                                            max: max_ord, 
                                            color: "#10b981".into() 
                                        }
                                    }
                                }
                            }
                            div class="analytics-card" {
                                h3 { "Status Distribution" }
                                div class="status-distribution" {
                                    let orders = state.orders.get();
                                    let mut counts: std::collections::HashMap<OrderStatus, usize> = std::collections::HashMap::new();
                                    for o in orders { *counts.entry(o.status.clone()).or_insert(0) += 1; }
                                    let total = orders.len() as f64;
                                    @for (status, count) in counts {
                                        div class="status-bar" {
                                            span { format!("{:?}", status) }
                                            div class="bar-container" {
                                                div class="bar-fill" style:width={ format!("{}%", (count as f64 / total) * 100.0) } {}
                                            }
                                            span { format!("{} ({:.0f}%)", count, (count as f64 / total) * 100.0) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                @if state.active_tab.get() == Tab::Activity {
                    div class="tab-content" {
                        h2 { "Activity Feed" }
                        div class="activity-list" {
                            @for activity in state.activities.get() {
                                ActivityItem { activity: activity.clone() }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// Entry Point
// ============================================================================

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    veloasm::mount(dashboard());
}

// For testing
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_compiles() {
        let _: DomNode = dashboard();
    }

    #[test]
    fn metric_card_compiles() {
        let _: DomNode = MetricCard { metric: mock_metrics()[0].clone() };
    }

    #[test]
    fn status_badge_compiles() {
        let _: DomNode = StatusBadge { status: OrderStatus::Delivered };
    }

    #[test]
    fn chart_bar_compiles() {
        let _: DomNode = ChartBar { label: "Test".into(), value: 100.0, max: 200.0, color: "#3b82f6".into() };
    }

    #[test]
    fn activity_item_compiles() {
        let _: DomNode = ActivityItem { activity: mock_activities()[0].clone() };
    }
}