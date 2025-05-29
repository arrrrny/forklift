# TensorZero Integration for Zed

This crate provides integration between Zed and TensorZero, allowing you to use TensorZero's model routing and optimization features within Zed's language model system.

## Overview

TensorZero is a platform for optimizing language model inference through intelligent routing, caching, and experimentation. This integration allows Zed to communicate with a local TensorZero instance using its OpenAI-compatible API.

## Configuration

### Prerequisites

1. Install and run TensorZero locally on `http://localhost:3000`
2. Configure your TensorZero instance with the models you want to use

### Zed Settings

Add TensorZero configuration to your Zed settings:

```json
{
  "language_models": {
    "tensor_zero": {
      "api_url": "http://localhost:3000/openai/v1",
      "available_models": [
        {
          "name": "tensorzero::model_name::openai::gpt_4o",
          "display_name": "GPT-4o",
          "max_tokens": 128000
        },
        {
          "name": "tensorzero::model_name::openai::gpt_4o_mini",
          "display_name": "GPT-4o Mini",
          "max_tokens": 128000
        },
        {
          "name": "tensorzero::model_name::anthropic::claude_3_5_sonnet",
          "display_name": "Claude 3.5 Sonnet",
          "max_tokens": 200000
        },
        {
          "name": "tensorzero::model_name::google_ai_studio::gemini_2_0_flash_exp",
          "display_name": "Gemini 2.0 Flash",
          "max_tokens": 1000000
        }
      ]
    }
  }
}
```

### Model Naming Convention

TensorZero uses a specific naming convention for models:

```
tensorzero::model_name::<provider>::<model_name>
```

Examples:
- `tensorzero::model_name::openai::gpt_4o`
- `tensorzero::model_name::anthropic::claude_3_5_sonnet`
- `tensorzero::model_name::google_ai_studio::gemini_2_0_flash_exp`
- `tensorzero::model_name::github_copilot::claude_sonnet_4`

## Features

- **Tool Support**: TensorZero models support function calling and tools by default
- **Streaming**: Real-time response streaming for interactive conversations
- **No Authentication**: Since TensorZero runs locally, no API keys are required
- **Model Routing**: TensorZero handles intelligent routing between different model providers
- **Optimization**: Benefit from TensorZero's inference optimization features

## Default Models

The integration provides two default models:
- **Default**: `tensorzero::model_name::openai::gpt_4o` (GPT-4o)
- **Fast**: `tensorzero::model_name::openai::gpt_4o_mini` (GPT-4o Mini)

## Configuration Options

### Available Model Settings

Each model in the `available_models` array supports:

- `name` (required): The TensorZero model identifier
- `display_name` (optional): Human-readable name shown in Zed's UI
- `max_tokens` (optional): Maximum context window size (defaults to 128000)

### API Settings

- `api_url` (optional): TensorZero API endpoint (defaults to `http://localhost:3000/openai/v1`)

## Usage

1. Start your TensorZero instance
2. Configure the available models in your Zed settings
3. Select TensorZero models from the language model picker in Zed
4. Use them like any other language model in Zed (Assistant panel, inline completions, etc.)

## Benefits of Using TensorZero

- **Cost Optimization**: Intelligent routing to the most cost-effective model for each task
- **Performance Optimization**: Caching and request optimization
- **A/B Testing**: Experiment with different models and configurations
- **Analytics**: Detailed insights into model usage and performance
- **Fallback Handling**: Automatic fallback to alternative models if one fails

## Troubleshooting

### TensorZero Not Running
- Ensure TensorZero is running on `http://localhost:3000`
- Check that the API endpoint is accessible

### Models Not Appearing
- Verify your `available_models` configuration in Zed settings
- Restart Zed after changing configuration
- Check that model names follow the correct TensorZero convention

### Connection Issues
- Confirm the `api_url` matches your TensorZero instance
- Check firewall settings if running on a different port
- Verify TensorZero is configured to accept connections