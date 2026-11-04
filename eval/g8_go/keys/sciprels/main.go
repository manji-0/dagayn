// Prints the relationships of every symbol a SCIP index describes as TSV:
// symbol, related symbol, flags (i: implementation, r: reference,
// t: type definition, d: definition).
package main

import (
	"bufio"
	"fmt"
	"os"

	"github.com/scip-code/scip/bindings/go/scip"
	"google.golang.org/protobuf/proto"
)

func flags(r *scip.Relationship) string {
	s := ""
	if r.IsImplementation {
		s += "i"
	}
	if r.IsReference {
		s += "r"
	}
	if r.IsTypeDefinition {
		s += "t"
	}
	if r.IsDefinition {
		s += "d"
	}
	return s
}

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
	emit := func(info *scip.SymbolInformation) {
		for _, r := range info.Relationships {
			fmt.Fprintf(w, "%s\t%s\t%s\n", info.Symbol, r.Symbol, flags(r))
		}
	}
	for _, doc := range index.Documents {
		for _, info := range doc.Symbols {
			emit(info)
		}
	}
	for _, info := range index.ExternalSymbols {
		emit(info)
	}
}
