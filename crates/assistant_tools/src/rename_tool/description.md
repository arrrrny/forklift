Renames a symbol across your codebase using the language server's semantic knowledge.

**How It Works**

This tool performs a rename refactoring operation on a specified symbol. It uses the project's language server to analyze the code and perform the rename correctly across all files where the symbol is referenced.

Unlike a simple find and replace, this tool understands the semantic meaning of the code, ensuring it only renames the specific symbol you specify and not unrelated text that happens to have the same name.

**Input Fields**
1. **Symbol Name** (Required): The name of the symbol to rename. This must be unique and identifiable within the codebase.
2. **New Name** (Required): The new name for the symbol. Ensure it adheres to naming conventions and does not conflict with existing symbols.
3. **File Path** (Optional): The relative path to the file containing the symbol. If omitted, the tool will search the entire codebase for the symbol.
4. **Context Before Symbol** (Optional): Text immediately preceding the symbol to help uniquely identify its location.
5. **Context After Symbol** (Optional): Text immediately following the symbol to ensure precise targeting.

**Examples of Symbols You Can Rename**
- Variables
- Functions
- Classes/structs
- Fields/properties
- Methods
- Interfaces/traits

The language server handles updating all references to the renamed symbol throughout the codebase, ensuring consistency and accuracy.
