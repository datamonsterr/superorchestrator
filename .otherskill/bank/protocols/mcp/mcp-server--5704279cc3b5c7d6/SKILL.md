---
name: mcp-server
description: Use when implementing, testing, or consuming Model Context Protocol (MCP) servers and clients in Python, integrating FastMCP or low-level mcp.ClientSession, handling streamable HTTP/SSE transports, managing tool discovery, evidence references, audit logs, and session-scoped caching.
---

# Model Context Protocol (MCP) Server & Client Guide

Comprehensive reference for building, securing, and integrating Model Context Protocol (MCP) servers and clients using the Python `mcp` SDK (`mcp>=2,<3`).

## When to Use This Skill

- Building custom MCP servers using FastMCP or low-level `mcp.server`.
- Connecting Python applications to remote MCP gateways via `mcp.ClientSession` and `streamable_http_client` or `stdio`.
- Implementing structured tool contracts, tool discovery, and runtime validation.
- Enforcing auditability, per-case isolation, and tracking `evidence_ref` tokens.
- Implementing client-side caching, connection pooling, and retry policies for high tool-efficiency scores.

---

## 1. MCP Architecture Overview

```text
┌───────────────────────┐                    ┌─────────────────────────┐
│       AI Agent        │                    │       MCP Server        │
│  (Coordinator/Worker) │                    │        (Gateway)        │
└──────────┬────────────┘                    └────────────┬────────────┘
           │                                              │
           │  1. initialize()                             │
           ├─────────────────────────────────────────────►│
           │  2. list_tools()                             │
           ├─────────────────────────────────────────────►│
           │  ◄── Tool Definitions & Schemas              │
           │                                              │
           │  3. call_tool(name, {case_id, args...})      │
           ├─────────────────────────────────────────────►│
           │  ◄── ToolResult(structuredContent / evidence)│
           │                                              │
           │  4. Emit Audit Event (Trace)                 │
           ▼                                              ▼
    Evidence Consumed                             Server Audit Log
```

---

## 2. Consuming MCP Tools with `ClientSession`

### Streamable HTTP Transport (Recommended for Gateways)

```python
import json
from contextlib import asynccontextmanager
from typing import Any, AsyncIterator
import httpx2
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client

class EvidenceGateway:
    def __init__(self, session: ClientSession):
        self._session = session
        self._cache: dict[str, Any] = {}

    async def list_tools(self) -> list[str]:
        response = await self._session.list_tools()
        return sorted(t.name for t in response.tools)

    async def call(self, tool_name: str, *, case_id: str, **kwargs) -> dict[str, Any]:
        """Calls an MCP tool with audit validation and case-level caching."""
        cache_key = f"{case_id}:{tool_name}:{sorted(kwargs.items())}"
        if cache_key in self._cache:
            return self._cache[cache_key]

        payload = {"case_id": case_id, **kwargs}
        result = await self._session.call_tool(tool_name, arguments=payload)
        
        if result.isError:
            err_msg = " ".join(b.text for b in result.content if getattr(b, "text", None))
            raise RuntimeError(f"MCP tool '{tool_name}' failed: {err_msg}")

        # Extract structured content or fall back to single JSON text block
        evidence = getattr(result, "structuredContent", None) or getattr(result, "structured_content", None)
        if evidence is None:
            text_blocks = [b.text for b in result.content if getattr(b, "text", None)]
            if len(text_blocks) != 1:
                raise ValueError(f"Tool '{tool_name}' did not return exactly one content block")
            evidence = json.loads(text_blocks[0])

        self._cache[cache_key] = evidence
        return evidence

@asynccontextmanager
async def connect_mcp_gateway(endpoint: str, api_key: str) -> AsyncIterator[EvidenceGateway]:
    headers = {"Authorization": f"Bearer {api_key}"}
    timeout = httpx2.Timeout(60.0, connect=10.0)
    async with (
        httpx2.AsyncClient(headers=headers, timeout=timeout) as http_client,
        streamable_http_client(endpoint, http_client=http_client) as (read_stream, write_stream),
        ClientSession(read_stream, write_stream) as session,
    ):
        await session.initialize()
        yield EvidenceGateway(session)
```

---

## 3. Building an MCP Server with FastMCP

For implementing a mock server, local development stub, or new microservice:

```python
from mcp.server.fastmcp import FastMCP
from pydantic import BaseModel, Field
import uuid

mcp = FastMCP("e-commerce-investigation-server")

class OrderEvidence(BaseModel):
    evidence_ref: str
    case_id: str
    status: str
    total_amount: float
    items: list[dict[str, Any]]

@mcp.tool()
async def get_order_details(order_id: str, case_id: str) -> dict[str, Any]:
    """Retrieve verified order records and generate an immutable evidence reference."""
    evidence_ref = f"ev_ord_{uuid.uuid4().hex[:12]}"
    
    # In production, query database / external API
    data = {
        "order_id": order_id,
        "status": "delivered",
        "total_amount": 149.90,
        "items": [{"product_id": "prod_123", "qty": 1, "price": 149.90}],
    }
    
    return {
        "evidence_ref": evidence_ref,
        "case_id": case_id,
        "source": "order_management_system",
        "data": data,
    }

if __name__ == "__main__":
    # Runs standard streamable HTTP or stdio server
    mcp.run()
```

---

## 4. MCP Audit, Provenance & Efficiency Rules

1. **Tool Discovery**: Always discover tools via `list_tools()` at initialization. Never hardcode unverified tool names.
2. **Case Scoping (`case_id`)**: Every tool call must include `case_id` for provenance auditing. If `case_id` is missing or mismatched, server calls will fail audit checks.
3. **Evidence References (`evidence_ref`)**:
   - `evidence_ref` is an immutable token generated by the MCP server.
   - Do NOT edit, mock, or manually craft `evidence_ref` in production workflows.
   - Record consumed `evidence_ref`s in the case trace via `trace.emit(event_type="tool_result_consumed", evidence_refs=[...])`.
4. **Efficiency & Anti-Fanout**:
   - Every unnecessary tool call reduces efficiency scoring.
   - Cache results per case (as shown in `EvidenceGateway`).
   - Do not query all tools blindly; inspect the complaint first to determine which domain specialists are needed.
