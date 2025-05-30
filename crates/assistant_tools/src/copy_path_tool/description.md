Copies a file or directory in the project and confirms success. Directory contents are copied recursively, similar to the behavior of a system copy command.

**Input Fields**
1. Source Path (Required): The relative path of the file or directory to copy. If a directory is specified, its contents will be copied recursively. For example, to copy a file located in directory1/a/something.txt, specify directory1/a/something.txt as the source path.

2. Destination Path (Required): The relative path where the file or directory should be copied to. For example, to copy directory1/a/something.txt to directory2/b/copy.txt, specify directory2/b/copy.txt as the destination path.

**Usage**
Use this tool to duplicate files or directories without modifying the original. It is faster and more efficient than manually reading and writing the contents, making it the preferred method for copying operations. Ensure both paths are valid and within the project boundaries. Both Source Path and Destination Path are mandatory fields for successful operation.
