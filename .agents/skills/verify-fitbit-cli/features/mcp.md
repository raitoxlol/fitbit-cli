# MCP

The helper starts the binary's stdio MCP server and sends initialize, tools/list,
and tools/call. Offline mode calls list_pulls. Live mode calls pull devices with
the requested account and checks isError=false plus a valid JSON array.
