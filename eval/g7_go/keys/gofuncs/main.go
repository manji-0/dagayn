// Lists every function and method declaration of a Go repository as TSV:
// path, name (Recv.Method for methods), first line, last line.
//
//	go run ./gofuncs REPO
package main

import (
	"bufio"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

func recvName(e ast.Expr) string {
	switch t := e.(type) {
	case *ast.StarExpr:
		return recvName(t.X)
	case *ast.IndexExpr:
		return recvName(t.X)
	case *ast.IndexListExpr:
		return recvName(t.X)
	case *ast.Ident:
		return t.Name
	}
	return "?"
}

func main() {
	root := os.Args[1]
	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	fset := token.NewFileSet()
	_ = filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			name := d.Name()
			if path != root && (strings.HasPrefix(name, ".") || name == "testdata" || strings.HasPrefix(name, "_")) {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(path, ".go") {
			return nil
		}
		f, err := parser.ParseFile(fset, path, nil, parser.SkipObjectResolution)
		if err != nil {
			return nil
		}
		rel, _ := filepath.Rel(root, path)
		for _, decl := range f.Decls {
			fn, ok := decl.(*ast.FuncDecl)
			if !ok {
				continue
			}
			name := fn.Name.Name
			if fn.Recv != nil && len(fn.Recv.List) > 0 {
				name = recvName(fn.Recv.List[0].Type) + "." + name
			}
			// Pos is the func keyword: a doc comment is not part of the definition.
			start := fset.Position(fn.Pos()).Line
			fmt.Fprintf(w, "%s\t%s\t%d\t%d\n", rel, name, start, fset.Position(fn.End()).Line)
		}
		return nil
	})
}
