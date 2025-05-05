// use std::sync::Arc;
// use anyhow::{Result, anyhow};
// use futures::{BoxFuture, Stream, StreamExt};
// use futures::stream::BoxStream;
// use gpui::{App, AsyncApp, Entity, Task, AnyView, Window};
// use language_model::{AuthenticateError, LanguageModel, LanguageModelCompletionError, LanguageModelCompletionEvent, LanguageModelId, LanguageModelName, LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName, LanguageModelProviderState, LanguageModelRequest, RateLimiter, Role, StopReason};
// use settings::SettingsStore;
// use ui::prelude::*;

// // Import existing providers
// use super::{deepseek::DeepSeekLanguageModelProvider, open_ai::OpenAiLanguageModelProvider, anthropic::AnthropicLanguageModelProvider};

// const PROVIDER_ID: &str = "zikzak_a1";
// const PROVIDER_NAME: &str = "ZikZak AI";
// const MODEL_ID: &str = "a1";
// const MODEL_NAME: &str = "A1";

// #[derive(Default, Clone, Debug, PartialEq)]
// pub struct ZikZakA1Settings {}

// pub struct ZikZakA1LanguageModelProvider {
//     deepseek: DeepSeekLanguageModelProvider,
//     openai: OpenAiLanguageModelProvider,
//     anthropic: AnthropicLanguageModelProvider,
//     state: Entity<State>,
// }

// pub struct State {
//     _settings_subscription: gpui::Subscription,
// }

// impl State {
//     fn is_authenticated(&self, cx: &App) -> bool {
//         // Authenticated if all underlying providers are authenticated
//         true // We'll refine this later
//     }
// }

// impl ZikZakA1LanguageModelProvider {
//     pub fn new(cx: &mut App) -> Self {
//         let deepseek = DeepSeekLanguageModelProvider::new(Default::default(), cx);
//         let openai = OpenAiLanguageModelProvider::new(cx);
//         let anthropic = AnthropicLanguageModelProvider::new(cx);
//         let state = cx.new(|cx| State {
//             _settings_subscription: cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
//         });
//         Self { deepseek, openai, anthropic, state }
//     }

//     fn create_language_model(&self) -> Arc<dyn LanguageModel> {
//         Arc::new(ZikZakA1LanguageModel {
//             deepseek: self.deepseek.clone(),
//             openai: self.openai.clone(),
//             anthropic: self.anthropic.clone(),
//             request_limiter: RateLimiter::new(4),
//         })
//     }
// }

// impl LanguageModelProviderState for ZikZakA1LanguageModelProvider {
//     type ObservableEntity = State;
//     fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
//         Some(self.state.clone())
//     }
// }

// impl LanguageModelProvider for ZikZakA1LanguageModelProvider {
//     fn id(&self) -> LanguageModelProviderId {
//         LanguageModelProviderId(PROVIDER_ID.into())
//     }
//     fn name(&self) -> LanguageModelProviderName {
//         LanguageModelProviderName(PROVIDER_NAME.into())
//     }
//     fn icon(&self) -> IconName {
//         IconName::Robot // Use a generic icon for now
//     }
//     fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
//         Some(self.create_language_model())
//     }
//     fn default_fast_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
//         Some(self.create_language_model())
//     }
//     fn provided_models(&self, _cx: &App) -> Vec<Arc<dyn LanguageModel>> {
//         vec![self.create_language_model()]
//     }
//     fn is_authenticated(&self, cx: &App) -> bool {
//         self.state.read(cx).is_authenticated(cx)
//     }
//     fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
//         // Authenticate all underlying providers
//         self.deepseek.authenticate(cx)
//     }
//     fn configuration_view(&self, window: &mut Window, cx: &mut App) -> AnyView {
//         // Show a simple config for now
//         div().child(Label::new("ZikZak A1 orchestrates DeepSeek, GPT-4.1, and Claude Sonnet.")).into()
//     }
//     fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
//         self.deepseek.reset_credentials(cx)
//     }
// }

// #[derive(Clone)]
// pub struct ZikZakA1LanguageModel {
//     deepseek: DeepSeekLanguageModelProvider,
//     openai: OpenAiLanguageModelProvider,
//     anthropic: AnthropicLanguageModelProvider,
//     request_limiter: RateLimiter,
// }

// impl ZikZakA1LanguageModel {
//     fn pick_backend(&self, request: &LanguageModelRequest) -> Backend {
//         // TODO: Replace with real heuristics
//         let prompt = request.messages.last().map(|m| m.string_contents()).unwrap_or_default();
//         if prompt.contains("create") || prompt.contains("design") {
//             Backend::DeepSeek
//         } else if prompt.len() < 200 {
//             Backend::OpenAI
//         } else {
//             Backend::Anthropic
//         }
//     }
// }

// enum Backend {
//     DeepSeek,
//     OpenAI,
//     Anthropic,
// }

// impl LanguageModel for ZikZakA1LanguageModel {
//     fn id(&self) -> LanguageModelId {
//         LanguageModelId::from(MODEL_ID)
//     }
//     fn name(&self) -> LanguageModelName {
//         LanguageModelName::from(MODEL_NAME)
//     }
//     fn provider_id(&self) -> LanguageModelProviderId {
//         LanguageModelProviderId(PROVIDER_ID.into())
//     }
//     fn provider_name(&self) -> LanguageModelProviderName {
//         LanguageModelProviderName(PROVIDER_NAME.into())
//     }
//     fn supports_tools(&self) -> bool {
//         true
//     }
//     fn telemetry_id(&self) -> String {
//         format!("zikzak_a1/{}", MODEL_ID)
//     }
//     fn max_token_count(&self) -> usize {
//         8192 // Arbitrary for now
//     }
//     fn count_tokens(&self, request: LanguageModelRequest, cx: &App) -> BoxFuture<'static, Result<usize>> {
//         // Use OpenAI's token counter for now
//         self.openai.default_model(cx).unwrap().count_tokens(request, cx)
//     }
//     fn stream_completion(&self, request: LanguageModelRequest, cx: &AsyncApp) -> BoxFuture<'static, Result<BoxStream<'static, Result<LanguageModelCompletionEvent, LanguageModelCompletionError>>>> {
//         let backend = self.pick_backend(&request);
//         match backend {
//             Backend::DeepSeek => {
//                 let model = self.deepseek.provided_models(&cx.as_app()).into_iter().find(|m| m.name().0.contains("Reasoner")).unwrap();
//                 model.stream_completion(request, cx)
//             }
//             Backend::OpenAI => {
//                 let model = self.openai.provided_models(&cx.as_app()).into_iter().find(|m| m.name().0.contains("4.1")).unwrap();
//                 model.stream_completion(request, cx)
//             }
//             Backend::Anthropic => {
//                 let model = self.anthropic.provided_models(&cx.as_app()).into
