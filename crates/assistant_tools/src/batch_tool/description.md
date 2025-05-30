This tool runs multiple operations either sequentially or concurrently, enabling efficient management of complex tasks.

**How It Works**
- Sequential Execution: Operations are performed in the exact order specified.
- Concurrent Execution: Operations are performed simultaneously, with no guaranteed order.

**Guidelines**
- Use this tool for two or more operations. For single operations, run them directly.
- Combine sequential and concurrent operations by nesting them.
- Limit batches to 32 operations for optimal performance.

**Examples**
- Sequential Example: Read a file, modify it, and save the changes in sequence.
- Concurrent Example: Search multiple files or directories simultaneously.

This tool ensures all operations in the batch have identical permissions and context as if executed individually. Results for each operation are provided upon completion.
