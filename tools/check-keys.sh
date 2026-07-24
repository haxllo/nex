echo "=== AgentRouter (OpenAI endpoint) ==="
curl -s -o /dev/null -w "%{http_code}" -X POST "https://agentrouter.org/v1/chat/completions" \
  -H "Authorization: Bearer sk-vKwyMIKB5jSDfBL8pQAEEPJk0N5H6bbrjKbok6AyZIlaPPC4" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5.5","messages":[{"role":"user","content":"test"}],"max_tokens":1}'
echo ""

echo "=== AgentRouter (Anthropic endpoint) ==="
curl -s -o /dev/null -w "%{http_code}" -X POST "https://agentrouter.org/v1/messages" \
  -H "x-api-key: sk-vKwyMIKB5jSDfBL8pQAEEPJk0N5H6bbrjKbok6AyZIlaPPC4" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-opus-4-8","max_tokens":1,"messages":[{"role":"user","content":"test"}]}'
echo ""

echo "=== OpenCode ==="
curl -s -o /dev/null -w "%{http_code}" -X POST "https://opencode.ai/zen/v1/chat/completions" \
  -H "Authorization: Bearer sk-3dJOQrk5sXqyk5MRMH8kMmHi6RSAiJSG6dpbCYyAOvrsKVudWkdNlvMcwuuT6ylj" \
  -H "Content-Type: application/json" \
  -d '{"model":"deepseek-v4-flash-free","messages":[{"role":"user","content":"test"}],"max_tokens":1}'
echo ""