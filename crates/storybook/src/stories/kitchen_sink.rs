use gpui::{prelude::*, Render, View};
use story::Story;
use strum::IntoEnumIterator;
use ui::prelude::*;

use crate::story_selector::ComponentStory;

pub struct KitchenSinkStory;

impl KitchenSinkStory {
    pub fn view(window: &mut gpui::Window, cx: &mut gpui::AppContext) -> View<Self> {
        cx.new_model(|_cx| Self)
    }
}

impl Render for KitchenSinkStory {
    fn render(&mut self, model: &Model<Self>, cx: &mut AppContext) -> impl IntoElement {
        let component_stories = ComponentStory::iter()
            .map(|selector| selector.story(cx))
            .collect::<Vec<_>>();

        Story::container()
            .id("kitchen-sink")
            .overflow_y_scroll()
            .child(Story::title("Kitchen Sink"))
            .child(Story::label("Components"))
            .child(div().flex().flex_col().children(component_stories))
            // Add a bit of space at the bottom of the kitchen sink so elements
            // don't end up squished right up against the bottom of the screen.
            .child(div().p_4())
    }
}
