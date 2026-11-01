//! Developer tools for inspecting and debugging the query cache.
//!
//! This module is only available when the `devtools` feature is enabled.

use crate::{QueryClient, QueryStateVariant};
use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, KeyBinding, Render, RenderOnce, SharedString,
    Styled, Window, actions, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, IconName, Sizable,
    button::{Button, ButtonVariants},
    clipboard::Clipboard,
    input::{Input, InputState},
    scroll::ScrollableElement,
    tab::{Tab, TabBar},
};
use std::rc::Rc;

actions!(
    devtools,
    [
        /// Toggle the visibility of the rs-query devtools panel.
        ToggleRsQueryDevtools,
    ]
);

/// Event types for timeline filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueryEventType {
    /// Every state update, regardless of kind (the default filter).
    All,
    /// First-load fetches (no data present before the fetch).
    Loading,
    /// Fetches that completed successfully with fresh data.
    Success,
    /// Fetches that failed.
    Error,
    /// Data transitions into (or is observed as) stale.
    Stale,
    /// Background refetches of already-cached data.
    Refetching,
}

impl QueryEventType {
    fn from_variant(variant: QueryStateVariant) -> Self {
        match variant {
            QueryStateVariant::Idle => Self::All,
            QueryStateVariant::Loading => Self::Loading,
            QueryStateVariant::Refetching => Self::Refetching,
            QueryStateVariant::Success => Self::Success,
            QueryStateVariant::Stale => Self::Stale,
            QueryStateVariant::Error => Self::Error,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All Events",
            Self::Loading => "Loading",
            Self::Success => "Success",
            Self::Error => "Error",
            Self::Stale => "Stale",
            Self::Refetching => "Refetching",
        }
    }
}

impl gpui_component::select::SelectItem for QueryEventType {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

/// Tab currently displayed in the devtools panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevtoolsTab {
    /// Live list of cached queries with per-entry actions.
    Queries,
    /// Chronological feed of observed state changes.
    Timeline,
    /// Raw cache inspector (placeholder).
    Cache,
}

/// A single timeline event.
#[derive(Clone)]
struct TimelineEvent {
    timestamp: chrono::DateTime<chrono::Local>,
    cache_key: String,
    variant: QueryStateVariant,
}

/// State for the devtools panel.
pub struct DevtoolsState {
    expanded: bool,
    selected_tab: DevtoolsTab,
    client: QueryClient,
    timeline_events: Vec<TimelineEvent>,
    timeline_search: Option<Entity<InputState>>,
    filter_event_type: QueryEventType,
    highlight_new_count: usize,
}

impl EventEmitter<()> for DevtoolsState {}

impl DevtoolsState {
    /// Create the devtools panel state and register its key bindings.
    pub fn new(client: QueryClient, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([KeyBinding::new("cmd-shift-q", ToggleRsQueryDevtools, None)]);
        cx.bind_keys([KeyBinding::new("ctrl-shift-q", ToggleRsQueryDevtools, None)]);

        // Poll the cache periodically to generate timeline events
        // cx.spawn(|this: WeakEntity<Self>, mut cx| async move {
        //     let mut interval = tokio::time::interval(Duration::from_millis(500));
        //     loop {
        //         interval.tick().await;
        //         _ = this.update(cx, |this: &mut Self, _cx| {
        //             this.update_timeline();
        //         });
        //     }
        // })
        // .detach();

        Self {
            expanded: true,
            selected_tab: DevtoolsTab::Queries,
            client,
            timeline_events: Vec::new(),
            timeline_search: None,
            filter_event_type: QueryEventType::All,
            highlight_new_count: 0,
        }
    }

    fn toggle_expanded(&mut self, cx: &mut Context<Self>) {
        self.expanded = !self.expanded;
        cx.notify();
    }

    fn set_selected_tab(&mut self, tab: DevtoolsTab, cx: &mut Context<Self>) {
        self.selected_tab = tab;
        cx.notify();
    }

    fn tab_index(&self) -> usize {
        match self.selected_tab {
            DevtoolsTab::Queries => 0,
            DevtoolsTab::Timeline => 1,
            DevtoolsTab::Cache => 2,
        }
    }

    fn tab_from_index(idx: usize) -> DevtoolsTab {
        match idx {
            1 => DevtoolsTab::Timeline,
            2 => DevtoolsTab::Cache,
            _ => DevtoolsTab::Queries,
        }
    }

    fn ensure_timeline_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.timeline_search.is_none() {
            let state = cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_placeholder("Search events...", window, cx);
                s
            });
            self.timeline_search = Some(state);
        }
    }

    fn filtered_events(&self, cx: &App) -> Vec<TimelineEvent> {
        let query = self
            .timeline_search
            .as_ref()
            .map(|s| s.read(cx).value().to_lowercase())
            .unwrap_or_default();
        self.timeline_events
            .iter()
            .filter(|e| {
                let event_type = QueryEventType::from_variant(e.variant);
                (self.filter_event_type == QueryEventType::All
                    || event_type == self.filter_event_type)
                    && (query.is_empty() || e.cache_key.to_lowercase().contains(&query))
            })
            .cloned()
            .collect()
    }

    fn clear_timeline(&mut self, cx: &mut Context<Self>) {
        self.timeline_events.clear();
        self.highlight_new_count = 0;
        cx.notify();
    }

    fn refetch_query(&self, key: &str) {
        let key = crate::QueryKey::new(key);
        self.client.invalidate_queries(&key);
    }

    fn invalidate_query(&self, key: &str) {
        let key = crate::QueryKey::new(key);
        self.client.invalidate_queries(&key);
    }

    fn remove_query(&self, key: &str) {
        self.client.cache.remove(key);
    }

    fn render_queries_tab(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = cx.theme();
        let mut queries: Vec<_> = self.client.cache.iter().collect();
        queries.sort_by(|a, b| a.key().cmp(b.key()));

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(theme.colors.accent)
                    .font_weight(FontWeight::MEDIUM)
                    .child(IconName::Folder)
                    .child("Active Queries"),
            )
            .child(
                div().flex_1().overflow_y_scrollbar().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(queries.into_iter().map(|entry| {
                            let key = entry.key().clone();
                            let entry = entry.value();
                            let variant = if entry.is_stale {
                                QueryStateVariant::Stale
                            } else {
                                QueryStateVariant::Success
                            };
                            let age = entry.fetched_at.elapsed();
                            let age_str = format!("{:.1}s ago", age.as_secs_f32());
                            Self::query_row(&key, variant, age_str, cx)
                        })),
                ),
            )
    }

    /// Render one cache-entry row: key, state variant, age and the
    /// refetch/invalidate/remove action buttons.
    fn query_row(
        key: &str,
        variant: QueryStateVariant,
        age_str: String,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .bg(theme.colors.secondary.opacity(0.5))
            .rounded(px(4.0))
            .hover(|s| s.bg(theme.colors.secondary))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().font_weight(FontWeight::MEDIUM).child(key.to_string()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.colors.muted_foreground)
                                    .child(format!("{variant:?}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.colors.muted_foreground)
                                    .child(age_str),
                            ),
                    )
                    .child(Self::query_row_actions(key, cx)),
            )
    }

    /// Render the refetch / invalidate / remove buttons for one cache entry.
    fn query_row_actions(key: &str, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .gap_2()
            .mt_1()
            .child(
                Button::new(format!("refetch-{key}"))
                    .label("Refetch")
                    .xsmall()
                    .ghost()
                    .debug_selector(move || format!("query-refetch-{key}"))
                    .on_click(cx.listener({
                        let key = key.to_string();
                        move |this, _, _, _| {
                            this.refetch_query(&key);
                        }
                    })),
            )
            .child(
                Button::new(format!("invalidate-{key}"))
                    .label("Invalidate")
                    .xsmall()
                    .ghost()
                    .debug_selector(move || format!("query-invalidate-{key}"))
                    .on_click(cx.listener({
                        let key = key.to_string();
                        move |this, _, _, _| {
                            this.invalidate_query(&key);
                        }
                    })),
            )
            .child(
                Button::new(format!("remove-{key}"))
                    .label("Remove")
                    .xsmall()
                    .ghost()
                    .debug_selector(move || format!("query-remove-{key}"))
                    .on_click(cx.listener({
                        let key = key.to_string();
                        move |this, _, _, _| {
                            this.remove_query(&key);
                        }
                    })),
            )
    }

    /// Lazily create the timeline type-filter select, wiring selection changes
    /// into [`Self::filter_event_type`].
    fn ensure_timeline_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<gpui_component::select::SelectState<Vec<QueryEventType>>> {
        self.ensure_timeline_search(window, cx);

        // Create filter state first, before any immutable borrow of cx.
        let items = vec![
            QueryEventType::All,
            QueryEventType::Loading,
            QueryEventType::Success,
            QueryEventType::Error,
            QueryEventType::Stale,
            QueryEventType::Refetching,
        ];
        let selected_index = items
            .iter()
            .position(|&t| t == self.filter_event_type)
            .map(gpui_component::IndexPath::new);
        let filter_state = cx
            .new(|cx| gpui_component::select::SelectState::new(items, selected_index, window, cx));
        let filter_state_clone = filter_state.clone();
        cx.subscribe(
            &filter_state,
            move |this: &mut Self,
                  _emitter: Entity<gpui_component::select::SelectState<_>>,
                  _event: &gpui_component::select::SelectEvent<_>,
                  cx| {
                if let Some(selected) = filter_state_clone.read(cx).selected_value().copied() {
                    this.filter_event_type = selected;
                    cx.notify();
                }
            },
        )
        .detach();
        filter_state
    }

    /// Render the timeline tab: filter toolbar, search input and the event
    /// list (newest first, newly recorded events highlighted).
    fn render_timeline_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let filter_state = self.ensure_timeline_filter(window, cx);

        // Now we can safely borrow cx immutably.
        let mut filtered = self.filtered_events(cx);
        filtered.reverse(); // newest first
        let item_count = filtered.len();
        let row_height = px(28.0);
        let row_width = px(2500.0);
        let item_sizes = Rc::new(vec![gpui::Size::new(row_width, row_height); item_count]);
        let search_entity = self.timeline_search.clone().unwrap();
        let copy_all_text = timeline_copy_text(&filtered);
        let highlight_count = self.highlight_new_count.min(item_count);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .size_full()
            .child(Self::timeline_toolbar(&filter_state, copy_all_text, cx))
            .child(
                Input::new(&search_entity)
                    .prefix(IconName::Search)
                    .cleanable(true)
                    .small(),
            )
            .child(if filtered.is_empty() {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("No events recorded yet"),
                    )
                    .into_any_element()
            } else {
                div()
                    .overflow_x_scrollbar()
                    .size_full()
                    .child(
                        gpui_component::v_virtual_list(
                            cx.entity().clone(),
                            "timeline-list",
                            item_sizes,
                            move |_view, visible_range, _window, cx| {
                                let events = filtered.clone();
                                visible_range
                                    .map(move |ix| {
                                        let event = &events[ix];
                                        let even = ix % 2 == 0;
                                        let is_new = ix < highlight_count;
                                        Self::timeline_row(
                                            event, even, is_new, row_width, row_height, cx,
                                        )
                                    })
                                    .collect()
                            },
                        )
                        .track_scroll(&gpui_component::VirtualListScrollHandle::new()),
                    )
                    .into_any_element()
            })
    }

    /// Render the timeline toolbar: tab title, type filter, copy-all and
    /// clear actions.
    fn timeline_toolbar(
        filter_state: &Entity<gpui_component::select::SelectState<Vec<QueryEventType>>>,
        copy_all_text: String,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(theme.colors.accent)
                    .font_weight(FontWeight::MEDIUM)
                    .child(IconName::Calendar)
                    .child("Event Timeline"),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        gpui_component::select::Select::new(filter_state)
                            .placeholder("Filter by type")
                            .small(),
                    )
                    .child(
                        Clipboard::new("copy-all-timeline")
                            .value(copy_all_text)
                            .tooltip("Copy all events"),
                    )
                    .child(
                        Button::new("clear-timeline")
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip("Clear timeline")
                            .debug_selector(|| "clear-timeline".to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clear_timeline(cx);
                            })),
                    ),
            )
    }

    /// Render one timeline row: timestamp, colored state variant and cache key.
    fn timeline_row(
        event: &TimelineEvent,
        even: bool,
        is_new: bool,
        row_width: gpui::Pixels,
        row_height: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let timestamp = event.timestamp.format("%H:%M:%S%.3f").to_string();
        let variant_str = format!("{:?}", event.variant);
        let theme = cx.theme();

        div()
            .w(row_width)
            .h(row_height)
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .bg(if is_new {
                theme.colors.accent.opacity(0.2)
            } else if even {
                theme.colors.background
            } else {
                theme.colors.secondary.opacity(0.3)
            })
            .hover(|style| style.bg(theme.colors.secondary.opacity(0.6)))
            .child(
                div()
                    .min_w(px(80.0))
                    .text_color(theme.colors.muted_foreground)
                    .font_family("monospace")
                    .text_sm()
                    .child(timestamp),
            )
            .child(
                div()
                    .min_w(px(80.0))
                    .text_color(match event.variant {
                        QueryStateVariant::Error
                        | QueryStateVariant::Stale
                        | QueryStateVariant::Success => theme.colors.accent,
                        _ => theme.colors.foreground,
                    })
                    .child(variant_str),
            )
            .child(div().flex_1().child(event.cache_key.clone()))
    }

    fn render_cache_tab(cx: &mut Context<Self>) -> gpui::Div {
        let theme = cx.theme();
        div()
            .gap_2()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(theme.colors.accent)
                    .font_weight(FontWeight::MEDIUM)
                    .child(IconName::File)
                    .child("Cache Inspector"),
            )
            .child(
                div()
                    .text_color(theme.colors.muted_foreground)
                    .child("Detailed cache inspection coming soon."),
            )
    }
}

/// Join filtered timeline events into one clipboard-friendly text block.
fn timeline_copy_text(events: &[TimelineEvent]) -> String {
    events
        .iter()
        .map(|e| {
            format!(
                "[{}] {} - {:?}",
                e.timestamp.format("%H:%M:%S%.3f"),
                e.cache_key,
                e.variant
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Render for DevtoolsState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.expanded {
            return Button::new("devtools-toggle")
                .icon(IconName::Settings)
                .rounded_full()
                .size(px(40.0))
                .shadow_md()
                .absolute()
                .bottom_3()
                .right_3()
                .on_click(cx.listener(|this, _, _, cx| this.toggle_expanded(cx)))
                .into_any_element();
        }

        let theme = cx.theme();
        let viewport = window.viewport_size();
        let panel_width = px(600.0).min(viewport.width - px(20.0));
        let panel_height = px(450.0).min(viewport.height - px(20.0));

        div()
            .absolute()
            .bottom_0()
            .right_0()
            .w(panel_width)
            .h(panel_height)
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .border_1()
            .border_color(theme.colors.border)
            .rounded_tl(px(8.0))
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .bg(theme.colors.secondary)
                    .rounded_tl(px(8.0))
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(IconName::Settings)
                            .child(" Query Devtools"),
                    )
                    .child(
                        Button::new("close-devtools")
                            .icon(IconName::Close)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_expanded(cx))),
                    ),
            )
            .child(
                TabBar::new("devtools-tabs")
                    .selected_index(self.tab_index())
                    .on_click(cx.listener(|this, index, _, cx| {
                        this.set_selected_tab(Self::tab_from_index(*index), cx);
                    }))
                    .child(
                        Tab::new()
                            .label("Queries")
                            .prefix(IconName::Folder)
                            .debug_selector(|| "tab-queries".to_string()),
                    )
                    .child(
                        Tab::new()
                            .label("Timeline")
                            .prefix(IconName::Calendar)
                            .debug_selector(|| "tab-timeline".to_string()),
                    )
                    .child(
                        Tab::new()
                            .label("Cache")
                            .prefix(IconName::File)
                            .debug_selector(|| "tab-cache".to_string()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .p_3()
                    .overflow_y_scrollbar()
                    .child(match self.selected_tab {
                        DevtoolsTab::Queries => self.render_queries_tab(cx).into_any_element(),
                        DevtoolsTab::Timeline => {
                            self.render_timeline_tab(window, cx).into_any_element()
                        }
                        DevtoolsTab::Cache => Self::render_cache_tab(cx).into_any_element(),
                    }),
            )
            .into_any_element()
    }
}

/// Root component rendering the devtools panel (or its collapsed toggle
/// button once rendered through [`Render`]).
#[derive(Clone)]
pub struct QueryDevtools {
    state: Entity<DevtoolsState>,
}

impl QueryDevtools {
    /// Mount the devtools panel for `client` into the app.
    pub fn new(client: QueryClient, cx: &mut App) -> Self {
        Self {
            state: cx.new(|cx| DevtoolsState::new(client, cx)),
        }
    }
}

impl RenderOnce for QueryDevtools {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(self.state)
    }
}

impl IntoElement for QueryDevtools {
    type Element = gpui::Component<Self>;

    fn into_element(self) -> Self::Element {
        gpui::Component::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryKey;
    use crate::QueryOptions;
    use gpui::{
        Empty, Modifiers, TestAppContext, VisualTestContext, WindowHandle, point, px, size,
    };
    use gpui_component::select::{SelectEvent, SelectItem};
    use std::time::Duration;

    struct DevtoolsHost;

    impl Render for DevtoolsHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            Empty
        }
    }

    fn make_event(cache_key: &str, variant: QueryStateVariant) -> TimelineEvent {
        TimelineEvent {
            timestamp: chrono::Local::now(),
            cache_key: cache_key.to_string(),
            variant,
        }
    }

    fn init_components(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
    }

    // ----- pure helpers (no GPUI context required) -----

    #[test]
    fn query_event_type_from_variant_maps_every_variant() {
        assert_eq!(
            QueryEventType::from_variant(QueryStateVariant::Idle),
            QueryEventType::All
        );
        assert_eq!(
            QueryEventType::from_variant(QueryStateVariant::Loading),
            QueryEventType::Loading
        );
        assert_eq!(
            QueryEventType::from_variant(QueryStateVariant::Refetching),
            QueryEventType::Refetching
        );
        assert_eq!(
            QueryEventType::from_variant(QueryStateVariant::Success),
            QueryEventType::Success
        );
        assert_eq!(
            QueryEventType::from_variant(QueryStateVariant::Stale),
            QueryEventType::Stale
        );
        assert_eq!(
            QueryEventType::from_variant(QueryStateVariant::Error),
            QueryEventType::Error
        );
    }

    #[test]
    fn query_event_type_labels_and_select_items() {
        assert_eq!(QueryEventType::All.label(), "All Events");
        assert_eq!(QueryEventType::Loading.label(), "Loading");
        assert_eq!(QueryEventType::Success.label(), "Success");
        assert_eq!(QueryEventType::Error.label(), "Error");
        assert_eq!(QueryEventType::Stale.label(), "Stale");
        assert_eq!(QueryEventType::Refetching.label(), "Refetching");

        for event_type in [
            QueryEventType::All,
            QueryEventType::Loading,
            QueryEventType::Success,
            QueryEventType::Error,
            QueryEventType::Stale,
            QueryEventType::Refetching,
        ] {
            assert_eq!(event_type.title(), event_type.label());
            assert_eq!(*event_type.value(), event_type);
        }
    }

    #[test]
    fn tab_from_index_maps_indices_with_fallback() {
        assert_eq!(DevtoolsState::tab_from_index(0), DevtoolsTab::Queries);
        assert_eq!(DevtoolsState::tab_from_index(1), DevtoolsTab::Timeline);
        assert_eq!(DevtoolsState::tab_from_index(2), DevtoolsTab::Cache);
        assert_eq!(DevtoolsState::tab_from_index(3), DevtoolsTab::Queries);
        assert_eq!(DevtoolsState::tab_from_index(999), DevtoolsTab::Queries);
    }

    #[test]
    fn timeline_copy_text_joins_events_and_handles_empty() {
        assert_eq!(timeline_copy_text(&[]), "");

        let events = vec![
            make_event("users::id=1", QueryStateVariant::Success),
            make_event("posts", QueryStateVariant::Error),
        ];
        let text = timeline_copy_text(&events);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("users::id=1"));
        assert!(lines[0].contains("Success"));
        assert!(lines[0].contains(" - "));
        assert!(lines[1].contains("posts"));
        assert!(lines[1].contains("Error"));

        // Clone derive sanity for the private event type.
        let cloned = events[0].clone();
        assert_eq!(cloned.cache_key, "users::id=1");
    }

    // ----- state helpers without a window -----

    #[gpui::test]
    fn state_defaults_and_toggle_tab_roundtrip(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let entity = cx.new(|cx| DevtoolsState::new(client, cx));

        entity.read_with(cx, |state, _| {
            assert!(state.expanded);
            assert_eq!(state.selected_tab, DevtoolsTab::Queries);
            assert_eq!(state.filter_event_type, QueryEventType::All);
            assert!(state.timeline_events.is_empty());
            assert!(state.timeline_search.is_none());
            assert_eq!(state.highlight_new_count, 0);
            assert_eq!(state.tab_index(), 0);
            assert_eq!(format!("{:?}", state.selected_tab), "Queries");
        });

        entity.update(cx, |state, cx| {
            state.toggle_expanded(cx);
            assert!(!state.expanded);
            state.toggle_expanded(cx);
            assert!(state.expanded);

            for (tab, index) in [
                (DevtoolsTab::Timeline, 1usize),
                (DevtoolsTab::Cache, 2),
                (DevtoolsTab::Queries, 0),
            ] {
                state.set_selected_tab(tab, cx);
                assert_eq!(state.selected_tab, tab);
                assert_eq!(state.tab_index(), index);
            }
        });
    }

    #[gpui::test]
    fn clear_timeline_drops_events_and_highlight_count(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let entity = cx.new(|cx| DevtoolsState::new(client, cx));

        entity.update(cx, |state, _| {
            state.timeline_events = vec![
                make_event("a", QueryStateVariant::Loading),
                make_event("b", QueryStateVariant::Success),
            ];
            state.highlight_new_count = 2;
        });
        entity.update(cx, super::DevtoolsState::clear_timeline);

        entity.read_with(cx, |state, _| {
            assert!(state.timeline_events.is_empty());
            assert_eq!(state.highlight_new_count, 0);
        });
    }

    #[gpui::test]
    fn filtered_events_respect_the_type_filter(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let entity = cx.new(|cx| DevtoolsState::new(client, cx));

        entity.update(cx, |state, _| {
            state.timeline_events = vec![
                make_event("users::1", QueryStateVariant::Loading),
                make_event("users::2", QueryStateVariant::Success),
                make_event("posts", QueryStateVariant::Error),
                make_event("teams", QueryStateVariant::Stale),
                make_event("orgs", QueryStateVariant::Refetching),
                make_event("misc", QueryStateVariant::Idle),
            ];
        });

        // No search input and the All filter: every event passes.
        assert_eq!(
            entity.read_with(cx, |state, app| state.filtered_events(app).len()),
            6
        );

        for (filter, expected_key) in [
            (QueryEventType::Loading, "users::1"),
            (QueryEventType::Success, "users::2"),
            (QueryEventType::Error, "posts"),
            (QueryEventType::Stale, "teams"),
            (QueryEventType::Refetching, "orgs"),
        ] {
            entity.update(cx, |state, _| state.filter_event_type = filter);
            let keys = entity.read_with(cx, |state, app| {
                state
                    .filtered_events(app)
                    .iter()
                    .map(|event| event.cache_key.clone())
                    .collect::<Vec<_>>()
            });
            assert_eq!(keys, vec![expected_key.to_string()], "filter {filter:?}");
        }

        // Idle events map to the All event type, so they only pass the All filter.
        entity.update(cx, |state, _| {
            state.filter_event_type = QueryEventType::Loading;
        });
        assert_eq!(
            entity.read_with(cx, |state, app| state.filtered_events(app).len()),
            1
        );
    }

    #[gpui::test]
    fn cache_actions_refetch_invalidate_and_remove(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let key = QueryKey::new("users").with("id", 1);
        client.set_query_data(&key, vec!["a".to_string()], QueryOptions::default());
        assert!(client.cache.contains_key("users::id=1"));
        assert!(!client.is_stale(&key));

        let entity = cx.new(|cx| DevtoolsState::new(client.clone(), cx));
        entity.update(cx, |state, _| {
            state.refetch_query("users");
            state.invalidate_query("users");
        });
        assert!(client.is_stale(&key));

        entity.update(cx, |state, _| state.remove_query("users::id=1"));
        assert!(!client.cache.contains_key("users::id=1"));

        // Actions on unknown keys are no-ops.
        entity.update(cx, |state, _| {
            state.refetch_query("missing");
            state.remove_query("missing");
        });
        assert!(client.cache.is_empty());
    }

    // ----- window-backed helpers and rendering -----

    #[gpui::test]
    fn timeline_search_input_filters_events_case_insensitively(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(client, cx));

        handle
            .update(cx, |state, window, cx| {
                state.timeline_events = vec![
                    make_event("users::1", QueryStateVariant::Loading),
                    make_event("users::2", QueryStateVariant::Success),
                    make_event("Posts", QueryStateVariant::Error),
                ];

                assert!(state.timeline_search.is_none());
                state.ensure_timeline_search(window, cx);
                let search = state.timeline_search.clone().expect("search input");
                state.ensure_timeline_search(window, cx);
                assert!(
                    state
                        .timeline_search
                        .as_ref()
                        .is_some_and(|created| created.entity_id() == search.entity_id())
                );
                search.update(cx, |input, input_cx| {
                    input.set_value("USERS", window, input_cx);
                });
            })
            .unwrap();

        let keys = handle
            .read_with(cx, |state, app| {
                state
                    .filtered_events(app)
                    .iter()
                    .map(|event| event.cache_key.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap();
        assert_eq!(keys, vec!["users::1".to_string(), "users::2".to_string()]);

        // Clearing the search restores every event.
        handle
            .update(cx, |state, window, cx| {
                let search = state.timeline_search.clone().expect("search input");
                search.update(cx, |input, input_cx| {
                    input.set_value("", window, input_cx);
                });
            })
            .unwrap();
        assert_eq!(
            handle
                .read_with(cx, |state, app| state.filtered_events(app).len())
                .unwrap(),
            3
        );
    }

    #[gpui::test]
    fn timeline_type_filter_select_updates_filter_event_type(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(client, cx));

        handle
            .update(cx, |state, window, cx| {
                state.timeline_events = vec![
                    make_event("users::1", QueryStateVariant::Loading),
                    make_event("posts", QueryStateVariant::Error),
                ];
                state.ensure_timeline_search(window, cx);
            })
            .unwrap();

        let filter = handle
            .update(cx, |state, window, cx| {
                state.ensure_timeline_filter(window, cx)
            })
            .unwrap();
        filter.read_with(cx, |select, _| {
            assert_eq!(
                *select.selected_value().expect("initial selection"),
                QueryEventType::All
            );
        });

        // Choosing an entry (index 4 == Stale) and confirming it propagates
        // into the devtools filter through the subscription.
        handle
            .update(cx, |_state, window, cx| {
                filter.update(cx, |select, select_cx| {
                    select.set_selected_index(
                        Some(gpui_component::IndexPath::new(4)),
                        window,
                        select_cx,
                    );
                });
            })
            .unwrap();
        filter.update(cx, |_select, select_cx| {
            select_cx.emit(SelectEvent::Confirm(Some(QueryEventType::Stale)));
        });
        cx.run_until_parked();

        handle
            .read_with(cx, |state, _| {
                assert_eq!(state.filter_event_type, QueryEventType::Stale);
            })
            .unwrap();

        // The filtered event list now only yields events of the selected type.
        let keys = handle
            .read_with(cx, |state, app| {
                state
                    .filtered_events(app)
                    .iter()
                    .map(|event| event.cache_key.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap();
        assert_eq!(keys, [] as [std::string::String; 0]);

        // Re-selecting the All entry (index 0) restores the unfiltered view.
        handle
            .update(cx, |_state, window, cx| {
                filter.update(cx, |select, select_cx| {
                    select.set_selected_index(
                        Some(gpui_component::IndexPath::new(0)),
                        window,
                        select_cx,
                    );
                });
            })
            .unwrap();
        filter.update(cx, |_select, select_cx| {
            select_cx.emit(SelectEvent::Confirm(Some(QueryEventType::All)));
        });
        cx.run_until_parked();
        assert_eq!(
            handle
                .read_with(cx, |state, app| state.filtered_events(app).len())
                .unwrap(),
            2
        );
    }

    #[gpui::test]
    fn timeline_tab_paints_virtual_list_rows(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(client, cx));

        // Paint the empty timeline first ("No events recorded yet").
        handle
            .update(cx, |state, _window, cx| {
                state.set_selected_tab(DevtoolsTab::Timeline, cx);
            })
            .unwrap();
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(1400.0), px(1000.0)),
            |_, _| view.into_any_element(),
        );

        // Paint again with recorded events so the virtual list renders rows.
        handle
            .update(cx, |state, _window, cx| {
                state.timeline_events = vec![
                    make_event("users::id=1", QueryStateVariant::Success),
                    make_event("posts", QueryStateVariant::Error),
                    make_event("teams", QueryStateVariant::Loading),
                    make_event("orgs", QueryStateVariant::Refetching),
                ];
                state.highlight_new_count = 1;
                cx.notify();
            })
            .unwrap();
        let view = handle.root(cx).unwrap();
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(1400.0), px(1000.0)),
            |_, _| view.into_any_element(),
        );
    }

    #[gpui::test]
    fn collapsed_state_paints_the_toggle_button(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(client, cx));

        handle
            .update(cx, |state, _window, cx| state.toggle_expanded(cx))
            .unwrap();
        assert!(!handle.read_with(cx, |state, _| state.expanded).unwrap());

        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        // The collapsed render path builds the floating toggle button.
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(800.0), px(600.0)),
            |_, _| view.into_any_element(),
        );
    }

    #[gpui::test]
    fn query_devtools_component_mounts_and_paints(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();

        // Mount the panel through the App-level constructor.
        let devtools = cx.update(|cx| QueryDevtools::new(client, cx));

        // Paint the component inside a host window (the host root view is the
        // otherwise-unused `DevtoolsHost` stub).
        let handle: WindowHandle<DevtoolsHost> = cx.add_window(|_window, _cx| DevtoolsHost);
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(800.0), px(600.0)),
            |_, _| {
                div()
                    .child(view.into_any_element())
                    .child(devtools)
                    .into_any_element()
            },
        );
    }

    #[gpui::test]
    fn cache_tab_paints_placeholder(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(client, cx));

        handle
            .update(cx, |state, _window, cx| {
                state.set_selected_tab(DevtoolsTab::Cache, cx);
            })
            .unwrap();
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(1400.0), px(1000.0)),
            |_, _| view.into_any_element(),
        );
    }

    #[gpui::test]
    fn toggle_action_binding_dispatches_via_keystrokes(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(client, cx));

        // The registered cmd/ctrl-shift-q bindings resolve to the
        // `ToggleRsQueryDevtools` action (no handler is attached, so the
        // dispatch is a no-op, but the action is built and matched).
        cx.simulate_keystrokes(*handle, "ctrl-shift-q");
        cx.run_until_parked();
        cx.simulate_keystrokes(*handle, "cmd-shift-q");
        cx.run_until_parked();

        handle
            .read_with(cx, |state, _| assert!(state.expanded))
            .unwrap();
    }

    // ----- hitbox-driven interactions through the real window pipeline -----
    //
    // These tests drive the painted panel the way the app does: the test app
    // draws the window itself (and redraws dirty windows on every
    // `run_until_parked`), so [`VisualTestContext::debug_bounds`] resolves
    // bounds recorded during the last completed frame and
    // [`VisualTestContext::simulate_click`] hit-tests against that frame.
    // Buttons and tabs expose deterministic debug selectors for this purpose.

    /// Resolve the painted bounds of an element by its debug selector.
    fn bounds_of(visual: &mut VisualTestContext, selector: &str) -> gpui::Bounds<gpui::Pixels> {
        let leaked: &'static str = Box::leak(selector.to_string().into_boxed_str());
        visual
            .debug_bounds(leaked)
            .unwrap_or_else(|| panic!("element `{selector}` should be painted"))
    }

    /// Click the center of an element previously painted under `selector`.
    fn click_selector(visual: &mut VisualTestContext, selector: &str) {
        let center = bounds_of(visual, selector).center();
        visual.simulate_click(center, Modifiers::default());
    }

    fn fresh_query_options() -> QueryOptions {
        QueryOptions {
            stale_time: Duration::from_secs(3600),
            ..QueryOptions::default()
        }
    }

    #[gpui::test]
    fn queries_tab_paints_fresh_and_stale_cache_rows(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let users = QueryKey::new("users").with("id", 1);
        let posts = QueryKey::new("posts");
        client.set_query_data(&users, "u".to_string(), fresh_query_options());
        client.set_query_data(&posts, "p".to_string(), fresh_query_options());

        // Invalidate before the first paint: the `users` row renders through
        // the Success branch while the `posts` row renders through the Stale
        // branch of the row mapper.
        client.invalidate_queries(&posts);
        assert!(client.is_stale(&posts));

        let cache_client = client.clone();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(cache_client, cx));
        cx.run_until_parked();

        let mut visual = VisualTestContext::from_window(*handle, cx);
        assert!(visual.debug_bounds("query-refetch-users::id=1").is_some());
        assert!(visual.debug_bounds("query-refetch-posts").is_some());

        // A state update repaints the panel and refreshes the recorded bounds.
        handle.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.run_until_parked();
        assert!(
            visual
                .debug_bounds("query-invalidate-users::id=1")
                .is_some()
        );
    }

    #[gpui::test]
    fn queries_tab_row_button_clicks_remove_entry_and_spare_other_rows(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let first = QueryKey::new("sweep-a").with("id", 1);
        let second = QueryKey::new("sweep-b").with("id", 2);
        client.set_query_data(&first, "a".to_string(), fresh_query_options());
        client.set_query_data(&second, "b".to_string(), fresh_query_options());

        let cache_client = client.clone();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(cache_client, cx));
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(*handle, cx);

        // The refetch and invalidate buttons mark the first entry stale while
        // leaving it cached.
        click_selector(&mut visual, "query-refetch-sweep-a::id=1");
        assert!(client.is_stale(&first));
        assert!(client.cache.contains_key("sweep-a::id=1"));

        click_selector(&mut visual, "query-invalidate-sweep-a::id=1");
        assert!(client.is_stale(&first));
        assert!(client.cache.contains_key("sweep-a::id=1"));

        // The remove button drops the entry from the cache...
        click_selector(&mut visual, "query-remove-sweep-a::id=1");
        assert!(!client.cache.contains_key("sweep-a::id=1"));

        // ...while the second row lies below it: untouched and still fresh.
        assert!(client.cache.contains_key("sweep-b::id=2"));
        assert!(!client.is_stale(&second));
    }

    #[gpui::test]
    fn queries_tab_refetch_button_click_marks_entry_stale(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let key = QueryKey::new("sliver").with("id", 1);
        client.set_query_data(&key, "v".to_string(), fresh_query_options());
        assert!(!client.is_stale(&key));

        let cache_client = client.clone();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(cache_client, cx));
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(*handle, cx);

        click_selector(&mut visual, "query-refetch-sliver::id=1");

        assert!(client.is_stale(&key));
        assert!(client.cache.contains_key("sliver::id=1"));
    }

    #[gpui::test]
    fn tab_bar_click_selects_the_clicked_tab(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(client, cx));
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(*handle, cx);

        click_selector(&mut visual, "tab-timeline");

        let selected = handle.read_with(cx, |state, _| state.selected_tab).unwrap();
        assert_eq!(selected, DevtoolsTab::Timeline);

        click_selector(&mut visual, "tab-cache");
        let selected = handle.read_with(cx, |state, _| state.selected_tab).unwrap();
        assert_eq!(selected, DevtoolsTab::Cache);

        click_selector(&mut visual, "tab-queries");
        let selected = handle.read_with(cx, |state, _| state.selected_tab).unwrap();
        assert_eq!(selected, DevtoolsTab::Queries);
    }

    #[gpui::test]
    fn clear_timeline_button_click_clears_recorded_events(cx: &mut TestAppContext) {
        init_components(cx);
        let client = QueryClient::new();
        let handle: WindowHandle<DevtoolsState> =
            cx.add_window(move |_window, cx| DevtoolsState::new(client, cx));

        handle
            .update(cx, |state, _window, cx| {
                state.set_selected_tab(DevtoolsTab::Timeline, cx);
                state.timeline_events = vec![
                    make_event("users::1", QueryStateVariant::Loading),
                    make_event("posts", QueryStateVariant::Success),
                ];
                state.highlight_new_count = 2;
            })
            .unwrap();
        cx.run_until_parked();

        let mut visual = VisualTestContext::from_window(*handle, cx);
        assert!(
            !handle
                .read_with(cx, |state, _| state.timeline_events.is_empty())
                .unwrap()
        );

        click_selector(&mut visual, "clear-timeline");

        assert!(
            handle
                .read_with(cx, |state, _| state.timeline_events.is_empty())
                .unwrap()
        );
        assert_eq!(
            handle
                .read_with(cx, |state, _| state.highlight_new_count)
                .unwrap(),
            0
        );
    }
}
