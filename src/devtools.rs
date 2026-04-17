//! Developer tools for inspecting and debugging the query cache.
//!
//! This module is only available when the `devtools` feature is enabled.

use crate::{QueryClient, QueryStateVariant};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, EventEmitter, FontWeight, KeyBinding,
    Render, RenderOnce, SharedString, Styled, WeakEntity, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    clipboard::Clipboard,
    input::{Input, InputState},
    scroll::ScrollableElement,
    tab::{Tab, TabBar},
    ActiveTheme, IconName, Sizable,
};
use std::rc::Rc;
use std::time::{Duration, Instant};

actions!(devtools, [ToggleDevtools]);

/// Event types for timeline filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueryEventType {
    All,
    Loading,
    Success,
    Error,
    Stale,
    Refetching,
}

impl QueryEventType {
    fn from_variant(variant: &QueryStateVariant) -> Self {
        match variant {
            QueryStateVariant::Idle => Self::All,
            QueryStateVariant::Loading => Self::Loading,
            QueryStateVariant::Refetching => Self::Refetching,
            QueryStateVariant::Success => Self::Success,
            QueryStateVariant::Stale => Self::Stale,
            QueryStateVariant::Error => Self::Error,
        }
    }

    fn label(&self) -> &'static str {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevtoolsTab {
    Queries,
    Timeline,
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
    last_snapshot: Vec<(String, QueryStateVariant, Instant)>,
    highlight_new_count: usize,
}

impl EventEmitter<()> for DevtoolsState {}

impl DevtoolsState {
    pub fn new(client: QueryClient, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([KeyBinding::new("cmd-shift-q", ToggleDevtools, None)]);
        cx.bind_keys([KeyBinding::new("ctrl-shift-q", ToggleDevtools, None)]);

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
            last_snapshot: Vec::new(),
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

    fn tab_from_index(&self, idx: usize) -> DevtoolsTab {
        match idx {
            0 => DevtoolsTab::Queries,
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

    fn update_timeline(&mut self) {
        let mut new_snapshot = Vec::new();
        for entry in self.client.cache.iter() {
            let key = entry.key().clone();
            let entry = entry.value();
            let variant = if entry.is_stale {
                QueryStateVariant::Stale
            } else {
                QueryStateVariant::Success
            };
            new_snapshot.push((key, variant, entry.fetched_at));
        }

        // Compare with last snapshot to detect changes
        for (key, variant, fetched_at) in &new_snapshot {
            if let Some((_, old_variant, old_fetched)) =
                self.last_snapshot.iter().find(|(k, _, _)| k == key)
            {
                if variant != old_variant || fetched_at != old_fetched {
                    self.timeline_events.push(TimelineEvent {
                        timestamp: chrono::Local::now(),
                        cache_key: key.clone(),
                        variant: variant.clone(),
                    });
                }
            } else {
                // New query
                self.timeline_events.push(TimelineEvent {
                    timestamp: chrono::Local::now(),
                    cache_key: key.clone(),
                    variant: variant.clone(),
                });
            }
        }

        self.last_snapshot = new_snapshot;
        self.highlight_new_count = self.timeline_events.len();
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
                let event_type = QueryEventType::from_variant(&e.variant);
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

    fn render_queries_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(key.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.colors.muted_foreground)
                                                        .child(format!("{:?}", variant)),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.colors.muted_foreground)
                                                        .child(age_str),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .gap_2()
                                                .mt_1()
                                                .child(
                                                    Button::new(format!("refetch-{}", key))
                                                        .label("Refetch")
                                                        .xsmall()
                                                        .ghost()
                                                        .on_click(cx.listener({
                                                            let key = key.clone();
                                                            move |this, _, _, _| {
                                                                this.refetch_query(&key);
                                                            }
                                                        })),
                                                )
                                                .child(
                                                    Button::new(format!("invalidate-{}", key))
                                                        .label("Invalidate")
                                                        .xsmall()
                                                        .ghost()
                                                        .on_click(cx.listener({
                                                            let key = key.clone();
                                                            move |this, _, _, _| {
                                                                this.invalidate_query(&key);
                                                            }
                                                        })),
                                                )
                                                .child(
                                                    Button::new(format!("remove-{}", key))
                                                        .label("Remove")
                                                        .xsmall()
                                                        .ghost()
                                                        .on_click(cx.listener({
                                                            let key = key.clone();
                                                            move |this, _, _, _| {
                                                                this.remove_query(&key);
                                                            }
                                                        })),
                                                ),
                                        ),
                                )
                        })),
                ),
            )
    }

    fn render_timeline_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                if let Some(selected) = filter_state_clone.read(cx).selected_value().cloned() {
                    this.filter_event_type = selected;
                    cx.notify();
                }
            },
        )
        .detach();

        // Now we can safely borrow cx immutably.
        let theme = cx.theme();
        let mut filtered = self.filtered_events(cx);
        filtered.reverse(); // newest first
        let item_count = filtered.len();
        let row_height = px(28.0);
        let row_width = px(2500.0);
        let item_sizes = Rc::new(vec![gpui::Size::new(row_width, row_height); item_count]);

        let search_entity = self.timeline_search.clone().unwrap();
        let copy_all_text = filtered
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
            .join("\n");

        let highlight_count = self.highlight_new_count.min(item_count);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .size_full()
            .child(
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
                                gpui_component::select::Select::new(&filter_state)
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
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_timeline(cx);
                                    })),
                            ),
                    ),
            )
            .child(
                Input::new(&search_entity)
                    .prefix(IconName::Search)
                    .cleanable(true)
                    .small(),
            )
            .child(
                div()
                    .flex_1()
                    .border_1()
                    .border_color(theme.colors.border)
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .child(if filtered.is_empty() {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_color(theme.colors.muted_foreground)
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
                                                let timestamp = event
                                                    .timestamp
                                                    .format("%H:%M:%S%.3f")
                                                    .to_string();
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
                                                    .hover(|style| {
                                                        style
                                                            .bg(theme.colors.secondary.opacity(0.6))
                                                    })
                                                    .child(
                                                        div()
                                                            .min_w(px(80.0))
                                                            .text_color(
                                                                theme.colors.muted_foreground,
                                                            )
                                                            .font_family("monospace")
                                                            .text_sm()
                                                            .child(timestamp),
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w(px(80.0))
                                                            .text_color(match event.variant {
                                                                QueryStateVariant::Error => {
                                                                    theme.colors.accent
                                                                }
                                                                QueryStateVariant::Stale => {
                                                                    theme.colors.accent
                                                                }
                                                                QueryStateVariant::Success => {
                                                                    theme.colors.accent
                                                                }
                                                                _ => theme.colors.foreground,
                                                            })
                                                            .child(variant_str),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .child(event.cache_key.clone()),
                                                    )
                                            })
                                            .collect()
                                    },
                                )
                                .track_scroll(&gpui_component::VirtualListScrollHandle::new()),
                            )
                            .into_any_element()
                    }),
            )
    }

    fn render_cache_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                        this.set_selected_tab(this.tab_from_index(*index), cx);
                    }))
                    .child(Tab::new().label("Queries").prefix(IconName::Folder))
                    .child(Tab::new().label("Timeline").prefix(IconName::Calendar))
                    .child(Tab::new().label("Cache").prefix(IconName::File)),
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
                        DevtoolsTab::Cache => self.render_cache_tab(cx).into_any_element(),
                    }),
            )
            .into_any_element()
    }
}

#[derive(Clone)]
pub struct QueryDevtools {
    state: Entity<DevtoolsState>,
}

impl QueryDevtools {
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
