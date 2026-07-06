# llm-gateway (delta)

本次变更 `refine-gateway-domain-and-parsing` 为纯内部重构，不新增、修改或移除任何需求级行为。`llm-gateway` 能力的全部现有需求保持不变。

## MODIFIED Requirements

（无。本次仅调整 `src/gateway/` 内部类型归属与请求体 JSON 解析路径，未改变任何 SHALL/MUST 级需求或场景。现有单元测试与集成测试全部保持通过，作为无行为回归的保证。）
