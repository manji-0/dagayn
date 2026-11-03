// Prints every occurrence of a SCIP index as TSV:
// path, line (1-based), start col, end line (1-based), end col, roles, symbol.
package main

import (
	"bufio"
	"fmt"
	"os"

	"github.com/scip-code/scip/bindings/go/scip"
	"google.golang.org/protobuf/proto"
)

func main() {
	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	var index scip.Index
	if err := proto.Unmarshal(data, &index); err != nil {
		panic(err)
	}
	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	for _, doc := range index.Documents {
		fmt.Fprintf(w, "#doc\t%s\t%d\n", doc.RelativePath, doc.PositionEncoding)
		for _, occ := range doc.Occurrences {
			r := occ.Range
			var sl, sc, el, ec int32
			if len(r) == 3 {
				sl, sc, el, ec = r[0], r[1], r[0], r[2]
			} else if len(r) == 4 {
				sl, sc, el, ec = r[0], r[1], r[2], r[3]
			} else {
				continue
			}
			fmt.Fprintf(w, "%s\t%d\t%d\t%d\t%d\t%d\t%s\n", doc.RelativePath, sl+1, sc, el+1, ec, occ.SymbolRoles, occ.Symbol)
		}
	}
	for _, sym := range index.ExternalSymbols {
		fmt.Fprintf(w, "#external\t%s\n", sym.Symbol)
	}
}
