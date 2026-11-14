package main

// The first-priority matcher (what Perl, RE2 and Go's regexp do by default): among the matches that start at the same place, the
// one the pattern lists first wins (`a|ab` on "ab" matches "a"; `a*` is greedy). A DFA state here is an ORDERED list of the NFA
// states still alive (the "threads"), best first. Each thread carries one register per capture boundary; a register is a plain
// local variable of the generated function, named by the thread's slot in its state's list, so the DFA's transitions say exactly
// which registers to copy and which to set to the current position. Nothing is allocated and nothing is searched at run time.

import (
	"fmt"
	"sort"
	"strings"
)

type seed struct{ nfa, from int }

type thr struct {
	nfa  int
	from int   // the slot of the thread (in the state it left) this one continues; -1 at the start
	tags []int // tags set on the way (in order: a later one for the same tag wins)
}

// the threads reached by following empty moves from the seeds, best first, deduplicated by NFA state (the first to arrive wins);
// a thread that reaches the accept state ends the list: everything after it has lower priority and can never win
func (m *nfa) pclosure(seeds []seed) []thr {
	visited := map[int]bool{}
	var out []thr
	stopped := false
	var visit func(s, from int, tags []int)
	visit = func(s, from int, tags []int) {
		if stopped || visited[s] {
			return
		}
		visited[s] = true
		st := &m.st[s]
		if st.isTag {
			tags = append(append([]int{}, tags...), st.tag)
		}
		if st.set != nil || st.accept {
			out = append(out, thr{s, from, tags})
			if st.accept {
				stopped = true
			}
			return
		}
		for _, e := range st.eps {
			visit(e, from, tags)
		}
	}
	for _, sd := range seeds {
		visit(sd.nfa, sd.from, nil)
	}
	return out
}

// what a transition does to the registers: new slot j takes slot `from`'s registers, then sets the tags in `tags`
type move struct {
	from int
	tags []int
}

type pdfa struct {
	next    [][256]int
	accept  []bool // the state's list ends in the accept state
	accSlot []int  // its slot, -1 if none
	moves   [][256][]move
	initial []move // threads of the start state (from is -1)
	slots   int    // the most threads in any state
	nTags   int
}

const maxRegisters = 600

func (d *pdfa) String() string { return fmt.Sprintf("%d states", len(d.next)) }

func buildPrio(tree *node, ngroups int, tags bool) (d *pdfa, err error) {
	defer func() {
		if r := recover(); r != nil {
			if s, ok := r.(string); ok {
				err = fmt.Errorf("%s", s)
				return
			}
			panic(r)
		}
	}()
	m := &nfa{}
	end := m.add()
	m.st[end].accept = true
	start := m.build(tree, end, tags)

	d = &pdfa{}
	if tags {
		d.nTags = 2 * ngroups
	}
	index := map[string]int{}
	var lists [][]int
	intern := func(ts []thr) int {
		var l []int
		for _, t := range ts {
			l = append(l, t.nfa)
		}
		k := fmt.Sprint(l)
		if i, ok := index[k]; ok {
			return i
		}
		if len(lists) >= maxStates {
			panic(fmt.Sprintf("more than %d states: simplify the pattern or split it", maxStates))
		}
		index[k] = len(lists)
		lists = append(lists, l)
		d.next = append(d.next, [256]int{})
		d.moves = append(d.moves, [256][]move{})
		slot := -1
		for j, s := range l {
			if m.st[s].accept {
				slot = j
			}
		}
		d.accept = append(d.accept, slot >= 0)
		d.accSlot = append(d.accSlot, slot)
		if len(l) > d.slots {
			d.slots = len(l)
		}
		if d.slots*d.nTags > maxRegisters {
			panic(fmt.Sprintf("%d registers needed for captures: simplify the pattern", d.slots*d.nTags))
		}
		return len(lists) - 1
	}
	first := m.pclosure([]seed{{start, -1}})
	for _, t := range first {
		d.initial = append(d.initial, move{-1, t.tags})
	}
	intern(first)
	for i := 0; i < len(lists); i++ {
		for b := 0; b < 256; b++ {
			var seeds []seed
			for k, s := range lists[i] {
				if m.st[s].set != nil && m.st[s].set[b] {
					seeds = append(seeds, seed{m.st[s].out, k})
				}
			}
			ts := m.pclosure(seeds)
			if len(ts) == 0 {
				d.next[i][b] = -1
				continue
			}
			d.next[i][b] = intern(ts)
			if tags {
				mv := make([]move, len(ts))
				for j, t := range ts {
					mv[j] = move{t.from, t.tags}
				}
				d.moves[i][b] = mv
			}
		}
	}
	return d, nil
}

// states that behave the same merge (only for a matcher without captures: with captures, registers tell states apart)
func (d *pdfa) minimize() *dfa {
	plain := &dfa{next: d.next, accept: d.accept}
	return plain.minimize()
}

func tagsKey(mv []move) string {
	if mv == nil {
		return ""
	}
	var b strings.Builder
	for _, m := range mv {
		fmt.Fprintf(&b, "%d:%v;", m.from, m.tags)
	}
	return b.String()
}

// ---- emitting

func emitHeader(b *strings.Builder, name, pattern string, what string) {
	fmt.Fprintf(b, "// %s: %s\n// %s\n", name, pattern, what)
}

// name_at(s, p): the end of the match starting at byte p, or -1
func emitAt(b *strings.Builder, name string, d *dfa) {
	fmt.Fprintf(b, "// %s_at(s, p): where the match starting at byte p ends (the pattern's first choice, greedy), or -1   (%d states)\n", name, len(d.next))
	fmt.Fprintf(b, "fn %s_at(s: text, p: i64) -> i64 {\n", name)
	b.WriteString("    var st = 0;\n    var i = p;\n    var last = -1;\n")
	if d.accept[0] {
		b.WriteString("    last = p;\n")
	}
	b.WriteString("    while i < len(s) {\n        let c = byte_at(s, i);\n        match st {\n")
	for s := range d.next {
		a := arms(d.next[s])
		if len(a) == 0 {
			continue
		}
		fmt.Fprintf(b, "            %d => {\n                match c {\n", s)
		for _, line := range a {
			fmt.Fprintf(b, "                    %s\n", line)
		}
		b.WriteString("                    else => { break; }\n                }\n            }\n")
	}
	b.WriteString("            else => { break; }\n        }\n        i += 1;\n")
	var acc []string
	for s, a := range d.accept {
		if a {
			acc = append(acc, fmt.Sprintf("st == %d", s))
		}
	}
	if len(acc) > 0 {
		fmt.Fprintf(b, "        if %s { last = i; }\n", strings.Join(acc, " || "))
	}
	b.WriteString("    }\n    return last;\n}\n")
}

// name_full(s): the whole text matches (the pattern as a language: does any way of matching reach the end?)
func emitFull(b *strings.Builder, name string, d *dfa) {
	fmt.Fprintf(b, "// %s_full(s): the whole text is one match   (%d states)\n", name, len(d.next))
	fmt.Fprintf(b, "fn %s_full(s: text) -> bool {\n    var st = 0;\n    var i = 0;\n    while i < len(s) {\n        let c = byte_at(s, i);\n        match st {\n", name)
	for s := range d.next {
		a := arms(d.next[s])
		if len(a) == 0 {
			continue
		}
		fmt.Fprintf(b, "            %d => {\n                match c {\n", s)
		for _, line := range a {
			fmt.Fprintf(b, "                    %s\n", line)
		}
		b.WriteString("                    else => { return false; }\n                }\n            }\n")
	}
	b.WriteString("            else => { return false; }\n        }\n        i += 1;\n    }\n")
	var acc []string
	for s, a := range d.accept {
		if a {
			acc = append(acc, fmt.Sprintf("st == %d", s))
		}
	}
	if len(acc) == 0 {
		b.WriteString("    return false;\n}\n")
	} else {
		fmt.Fprintf(b, "    return %s;\n}\n", strings.Join(acc, " || "))
	}
}

func emitFind(b *strings.Builder, name string) {
	fmt.Fprintf(b, "// %s_find(s, from): where the first match at or after byte `from` starts, or -1; its end is %s_at(s, start)\n", name, name)
	fmt.Fprintf(b, "fn %s_find(s: text, from: i64) -> i64 {\n    var p = from;\n    while p <= len(s) {\n        if %s_at(s, p) >= 0 { return p; }\n        p += 1;\n    }\n    return -1;\n}\n", name, name)
}

// name_match(s, p, m): like name_at, and fills m with where each group matched (m.lo[g], m.hi[g]; group 0 is the whole match; -1 for a group that took no part)
func emitCaps(b *strings.Builder, name string, ngroups int, d *pdfa) {
	T := d.nTags
	fmt.Fprintf(b, "struct %s_m { lo: i64[%d]; hi: i64[%d]; }\n", name, ngroups+1, ngroups+1)
	fmt.Fprintf(b, "// %s_match(s, p, m): the match starting at byte p, as %s_at, with its groups: m.lo[g]..m.hi[g] for group g (0 is the whole match, -1 a group that took no part). True when there is a match.   (%d states, %d registers)\n", name, name, len(d.next), d.slots*T)
	fmt.Fprintf(b, "fn %s_match(s: text, p: i64, m: write %s_m) -> bool {\n", name, name)
	b.WriteString("    var st = 0;\n    var i = p;\n    var xend = -1;\n")
	for t := 0; t < T; t++ {
		fmt.Fprintf(b, "    var x%d = -1;\n", t)
	}
	for j := 0; j < d.slots; j++ {
		for t := 0; t < T; t++ {
			fmt.Fprintf(b, "    var r%d_%d = -1;\n", j, t)
		}
	}
	for j, mv := range d.initial {
		for _, t := range mv.tags {
			fmt.Fprintf(b, "    r%d_%d = p;\n", j, t)
		}
	}
	record := func(indent string, slot int, pos string) {
		fmt.Fprintf(b, "%sxend = %s;\n", indent, pos)
		for t := 0; t < T; t++ {
			fmt.Fprintf(b, "%sx%d = r%d_%d;\n", indent, t, slot, t)
		}
	}
	if d.accept[0] {
		record("    ", d.accSlot[0], "p")
	}
	b.WriteString("    while i < len(s) {\n        let c = byte_at(s, i);\n        match st {\n")
	for s := range d.next {
		// bytes with the same target and the same register moves share an arm
		type group struct {
			ranges []string
			target int
			mv     []move
		}
		var order []string
		groups := map[string]*group{}
		for c := 0; c < 256; {
			t := d.next[s][c]
			key := fmt.Sprintf("%d|%s", t, tagsKey(d.moves[s][c]))
			e := c
			for e+1 < 256 && d.next[s][e+1] == t && tagsKey(d.moves[s][e+1]) == tagsKey(d.moves[s][c]) {
				e++
			}
			if t >= 0 {
				g, ok := groups[key]
				if !ok {
					g = &group{target: t, mv: d.moves[s][c]}
					groups[key] = g
					order = append(order, key)
				}
				if e == c {
					g.ranges = append(g.ranges, fmt.Sprint(c))
				} else {
					g.ranges = append(g.ranges, fmt.Sprintf("%d..%d", c, e))
				}
			}
			c = e + 1
		}
		if len(order) == 0 {
			continue
		}
		sort.SliceStable(order, func(a, b int) bool { return groups[order[a]].target < groups[order[b]].target })
		fmt.Fprintf(b, "            %d => {\n                match c {\n", s)
		for _, key := range order {
			g := groups[key]
			fmt.Fprintf(b, "                    %s => {\n", strings.Join(g.ranges, ", "))
			emitMoves(b, "                        ", g.mv, T)
			fmt.Fprintf(b, "                        st = %d;\n                    }\n", g.target)
		}
		b.WriteString("                    else => { break; }\n                }\n            }\n")
	}
	b.WriteString("            else => { break; }\n        }\n        i += 1;\n")
	for s, a := range d.accept {
		if a {
			fmt.Fprintf(b, "        if st == %d {\n", s)
			record("            ", d.accSlot[s], "i")
			b.WriteString("        }\n")
		}
	}
	b.WriteString("    }\n    if xend < 0 { return false; }\n    m.lo[0] = p;\n    m.hi[0] = xend;\n")
	for g := 1; g <= ngroups; g++ {
		fmt.Fprintf(b, "    m.lo[%d] = x%d;\n    m.hi[%d] = x%d;\n", g, 2*(g-1), g, 2*(g-1)+1)
	}
	b.WriteString("    return true;\n}\n")
	fmt.Fprintf(b, "// %s_search(s, from, m): the first match at or after byte `from`, with its groups in m; false if there is none\n", name)
	fmt.Fprintf(b, "fn %s_search(s: text, from: i64, m: write %s_m) -> bool {\n    var p = from;\n    while p <= len(s) {\n        if %s_match(s, p, m) { return true; }\n        p += 1;\n    }\n    return false;\n}\n", name, name, name)
}

// the register moves of one transition: every copy reads the registers as they were before the byte (through temporaries), then the tags are set to the position after it
func emitMoves(b *strings.Builder, indent string, mv []move, nT int) {
	type cp struct{ j, k, t int }
	var copies []cp
	var sets [][2]int
	for j, m := range mv {
		set := map[int]bool{}
		for _, t := range m.tags {
			set[t] = true
		}
		var ts []int
		for t := range set {
			ts = append(ts, t)
		}
		sort.Ints(ts)
		for _, t := range ts {
			sets = append(sets, [2]int{j, t})
		}
		for t := 0; t < nT; t++ {
			if set[t] || (m.from == j) {
				continue
			}
			copies = append(copies, cp{j, m.from, t})
		}
	}
	for _, c := range copies {
		fmt.Fprintf(b, "%slet n%d_%d = r%d_%d;\n", indent, c.j, c.t, c.k, c.t)
	}
	for _, c := range copies {
		fmt.Fprintf(b, "%sr%d_%d = n%d_%d;\n", indent, c.j, c.t, c.j, c.t)
	}
	for _, s := range sets {
		fmt.Fprintf(b, "%sr%d_%d = i + 1;\n", indent, s[0], s[1])
	}
}
