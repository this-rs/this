#!/bin/bash
# Script pour afficher l'arborescence du projet

echo "📦 This-RS Project Structure"
echo "================================"
echo ""

if command -v tree &> /dev/null; then
    tree -I 'target' -L 3
else
    find . -type f -o -type d | grep -v target | sort | sed 's|^\./||' | sed 's|[^/]*/|  |g'
fi

echo ""
echo "📊 Statistics:"
echo "--------------------------------"
echo "Rust files:     $(find . -name "*.rs" | wc -l)"
echo "Total files:    $(find . -type f | grep -v target | wc -l)"
echo "Lines of code:  $(find . -name "*.rs" -exec cat {} \; | wc -l)"
