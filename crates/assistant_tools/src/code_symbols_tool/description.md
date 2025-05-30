This tool provides detailed information about code symbols in your project, such as variables, functions, classes, interfaces, traits, and other programming constructs, using Language Server Protocol (LSP) integration.

**Input Fields**:

**Required Fields**:
1. `path`: The relative path to the file containing the symbol. This must start with one of the project's root directories.
   - Example: `src/main.rs`
2. `command`: The type of information to retrieve about the symbol. Options include:
   - `Definition`: Find where the symbol is first assigned.
   - `Declaration`: Find where the symbol is first declared.
   - `Implementation`: Find the symbol's implementation.
   - `TypeDefinition`: Retrieve the symbol's type definition.
   - `References`: Locate all references to the symbol across the project.
3. `symbol`: The name of the symbol to query. This must appear between `context_before_symbol` and `context_after_symbol`.

**Optional Fields**:
4. `context_before_symbol`: Text that appears immediately before the symbol in the file. This helps uniquely identify the symbol's location.
5. `context_after_symbol`: Text that appears immediately after the symbol in the file. This ensures the query is precise and unique.

**Usage**:
- Use this tool to retrieve precise information about code symbols, such as their definitions, declarations, implementations, type definitions, or references.
- Provide sufficient context before and after the symbol to ensure the query is unique and accurate.

**Advantages**:
- This tool is more reliable than regex searches because it accounts for semantics like aliases.
- Use this tool for precise information about code symbols instead of textual search tools.

**Limitations**:
- Do not use this tool for searching non-symbol-related content.
