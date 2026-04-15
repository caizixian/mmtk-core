import os
import re

unsafe_pattern = re.compile(r"\bunsafe\s*\{|\bunsafe\s+fn\b|\bunsafe\s+impl\b")
comment_pattern = re.compile(r"^\s*//")

files_with_unsafe = []

for root, dirs, files in os.walk("src"):
    for file in files:
        if file.endswith(".rs"):
            path = os.path.join(root, file)
            try:
                with open(path, "r") as f:
                    for line in f:
                        if comment_pattern.match(line):
                            continue
                        if unsafe_pattern.search(line):
                            files_with_unsafe.append(path)
                            break
            except Exception as e:
                print(f"Error reading {path}: {e}")

print("Found files with unsafe:")
for p in files_with_unsafe:
    print(p)
