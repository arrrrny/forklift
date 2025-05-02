#!/bin/bash
cd $(dirname $0)/../../
echo "Running cargo check on open_router crate..."
cargo check --package open_router
if [ $? -eq 0 ]; then
    echo "✅ VICTORY! ALL BUGS CRUSHED!"
    echo "OpenRouter crate compiles successfully with zero errors!"
else
    echo "❌ Still some bugs to crush! Keep going!"
fi