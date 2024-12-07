use client::{ContactRequestStatus, User, UserStore};
use gpui::{
    AppContext, DismissEvent, EventEmitter, FocusHandle, FocusableView, Model, ModelContext,
    ParentElement as _, Render, Styled, Task, View, VisualContext, WeakView,
};
use picker::{Picker, PickerDelegate};
use std::sync::Arc;
use ui::{prelude::*, Avatar, ListItem, ListItemSpacing};
use util::{ResultExt as _, TryFutureExt};
use workspace::ModalView;

pub struct ContactFinder {
    picker: Model<Picker<ContactFinderDelegate>>,
}

impl ContactFinder {
    pub fn new(user_store: Model<UserStore>, cx: &mut ModelContext<Self>) -> Self {
        let delegate = ContactFinderDelegate {
            parent: cx.handle().downgrade(),
            user_store,
            potential_contacts: Arc::from([]),
            selected_index: 0,
        };
        let picker = cx.new_model(|cx| Picker::uniform_list(delegate, cx).modal(false));

        Self { picker }
    }

    pub fn set_query(&mut self, query: String, window: &mut Window, cx: &mut ModelContext<Self>) {
        self.picker.update(window, cx, |picker, cx| {
            picker.set_query(query, window, cx);
        });
    }
}

impl Render for ContactFinder {
    fn render(&mut self, cx: &mut ModelContext<Self>) -> impl IntoElement {
        v_flex()
            .elevation_3(cx)
            .child(
                v_flex()
                    .px_2()
                    .py_1()
                    .bg(cx.theme().colors().element_background)
                    // HACK: Prevent the background color from overflowing the parent container.
                    .rounded_t(px(8.))
                    .child(Label::new("Contacts"))
                    .child(h_flex().child(Label::new("Invite new contacts"))),
            )
            .child(self.picker.clone())
            .w(rems(34.))
    }
}

pub struct ContactFinderDelegate {
    parent: WeakModel<ContactFinder>,
    potential_contacts: Arc<[Arc<User>]>,
    user_store: Model<UserStore>,
    selected_index: usize,
}

impl EventEmitter<DismissEvent> for ContactFinder {}
impl ModalView for ContactFinder {}

impl FocusableView for ContactFinder {
    fn focus_handle(&self, cx: &AppContext) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl PickerDelegate for ContactFinderDelegate {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.potential_contacts.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut ModelContext<Picker<Self>>) {
        self.selected_index = ix;
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut AppContext) -> Arc<str> {
        "Search collaborator by username...".into()
    }

    fn update_matches(&mut self, query: String, cx: &mut ModelContext<Picker<Self>>) -> Task<()> {
        let search_users = self
            .user_store
            .update(cx, |store, cx| store.fuzzy_search_users(query, cx));

        cx.spawn(|picker, mut cx| async move {
            async {
                let potential_contacts = search_users.await?;
                picker.update(&mut cx, |picker, cx| {
                    picker.delegate.potential_contacts = potential_contacts.into();
                    cx.notify();
                })?;
                anyhow::Ok(())
            }
            .log_err()
            .await;
        })
    }

    fn confirm(&mut self, _: bool, cx: &mut ModelContext<Picker<Self>>) {
        if let Some(user) = self.potential_contacts.get(self.selected_index) {
            let user_store = self.user_store.read(cx);
            match user_store.contact_request_status(user) {
                ContactRequestStatus::None | ContactRequestStatus::RequestReceived => {
                    self.user_store
                        .update(cx, |store, cx| store.request_contact(user.id, cx))
                        .detach();
                }
                ContactRequestStatus::RequestSent => {
                    self.user_store
                        .update(cx, |store, cx| store.remove_contact(user.id, cx))
                        .detach();
                }
                _ => {}
            }
        }
    }

    fn dismissed(&mut self, cx: &mut ModelContext<Picker<Self>>) {
        self.parent
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .log_err();
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        cx: &mut ModelContext<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let user = &self.potential_contacts[ix];
        let request_status = self.user_store.read(cx).contact_request_status(user);

        let icon_path = match request_status {
            ContactRequestStatus::None | ContactRequestStatus::RequestReceived => {
                Some("icons/check.svg")
            }
            ContactRequestStatus::RequestSent => Some("icons/x.svg"),
            ContactRequestStatus::RequestAccepted => None,
        };
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .selected(selected)
                .start_slot(Avatar::new(user.avatar_uri.clone()))
                .child(Label::new(user.github_login.clone()))
                .end_slot::<Icon>(icon_path.map(Icon::from_path)),
        )
    }
}
