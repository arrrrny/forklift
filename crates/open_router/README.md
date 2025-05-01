# openrouter

OpenRouter language model provider for Zed, API-compatible with OpenAI.

- Implements the same API as OpenAI, but targets https://openrouter.ai
- Supports models:
  - google/gemini-2.0-flash-exp:free (1M tokens, tools)
  - google/gemini-2.5-pro-exp-03-25 (1M tokens, tools)
  - qwen/qwen-turbo (1M tokens, tools)
  - meta-llama/llama-4-scout (128K tokens, tools)
  - qwen/qwen3-235b-a22b (tools)

## Usage

Add to your workspace and use as a drop-in replacement for OpenAI provider.

## License

GPL-3.0-or-later
