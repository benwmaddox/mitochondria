// regexgen: turns a file of named patterns into Mitochondria functions.
//
//	go run . patterns.rx > patterns.mito && mito fmt patterns.mito --write
//
// (the formatter step keeps the file in the repository's style; the generated code is the same either way)
//
// Each line of the input is `name  pattern` (blank lines and lines starting with // are skipped). A pattern is compiled to a DFA
// when this tool runs, so the generated code is plain loops over bytes: no allocation, no stack, time linear in the text.
//
// Syntax: literals, `.` (any byte but newline), classes `[a-z_]` `[^"]`, escapes `\d \w \s \D \W \S \n \t \xHH` and `\` before
// punctuation, `( )`, `|`, `* + ?` and `{m}` `{m,}` `{m,n}`. Not supported, on purpose: anchors (use name_full for a whole match),
// backreferences, lookaround, captures (a DFA does not track them). Matching is on bytes: a negated class matches any other byte.
package main

import (
	"fmt"
	"os"
	"sort"
	"strings"
)

// ---- pattern syntax tree

type kind int

const (
	kSet kind = iota // one byte from a set
	kCat
	kAlt
	kStar
	kPlus
	kOpt
	kEmpty
	kGroup // a capture: subs[0], numbered group
)

type node struct {
	k    kind
	set  [256]bool
	subs []*node
	grp  int
}

type parser struct {
	s       string
	pos     int
	ngroups int
}

func (p *parser) more() bool { return p.pos < len(p.s) }
func (p *parser) peek() byte { return p.s[p.pos] }

func (p *parser) fail(format string, a ...any) {
	panic(fmt.Sprintf("column %d: %s", p.pos+1, fmt.Sprintf(format, a...)))
}

func (p *parser) alt() *node {
	subs := []*node{p.cat()}
	for p.more() && p.peek() == '|' {
		p.pos++
		subs = append(subs, p.cat())
	}
	if len(subs) == 1 {
		return subs[0]
	}
	return &node{k: kAlt, subs: subs}
}

func (p *parser) cat() *node {
	var subs []*node
	for p.more() && p.peek() != '|' && p.peek() != ')' {
		subs = append(subs, p.rep())
	}
	if len(subs) == 0 {
		return &node{k: kEmpty}
	}
	if len(subs) == 1 {
		return subs[0]
	}
	return &node{k: kCat, subs: subs}
}

func (p *parser) rep() *node {
	n := p.atom()
	for p.more() {
		switch p.peek() {
		case '*':
			p.pos++
			n = &node{k: kStar, subs: []*node{n}}
		case '+':
			p.pos++
			n = &node{k: kPlus, subs: []*node{n}}
		case '?':
			p.pos++
			n = &node{k: kOpt, subs: []*node{n}}
		case '{':
			n = p.counted(n)
		default:
			return n
		}
	}
	return n
}

func (p *parser) number() int {
	start := p.pos
	v := 0
	for p.more() && p.peek() >= '0' && p.peek() <= '9' {
		v = v*10 + int(p.peek()-'0')
		p.pos++
		if v > 100 {
			p.fail("a repeat count above 100")
		}
	}
	if p.pos == start {
		p.fail("expected a number")
	}
	return v
}

// x{m}, x{m,}, x{m,n}: copies of x (the tree is shared, which is fine: it is only read)
func (p *parser) counted(n *node) *node {
	p.pos++ // {
	lo := p.number()
	hi := lo
	if p.more() && p.peek() == ',' {
		p.pos++
		if p.more() && p.peek() == '}' {
			hi = -1
		} else {
			hi = p.number()
		}
	}
	if !p.more() || p.peek() != '}' {
		p.fail("expected }")
	}
	p.pos++
	if hi >= 0 && hi < lo {
		p.fail("{%d,%d}: the top is below the bottom", lo, hi)
	}
	var subs []*node
	for i := 0; i < lo; i++ {
		subs = append(subs, n)
	}
	if hi < 0 {
		subs = append(subs, &node{k: kStar, subs: []*node{n}})
	} else {
		for i := lo; i < hi; i++ {
			subs = append(subs, &node{k: kOpt, subs: []*node{n}})
		}
	}
	if len(subs) == 0 {
		return &node{k: kEmpty}
	}
	return &node{k: kCat, subs: subs}
}

func (p *parser) atom() *node {
	c := p.peek()
	switch c {
	case '(':
		p.pos++
		group := true
		if p.pos+1 < len(p.s) && p.s[p.pos] == '?' && p.s[p.pos+1] == ':' {
			group = false
			p.pos += 2
		} else if p.more() && p.peek() == '?' {
			p.fail("only (?: is supported after a (")
		}
		g := 0
		if group {
			p.ngroups++
			g = p.ngroups
		}
		n := p.alt()
		if !p.more() || p.peek() != ')' {
			p.fail("a ( is never closed")
		}
		p.pos++
		if group {
			return &node{k: kGroup, subs: []*node{n}, grp: g}
		}
		return n
	case '[':
		return p.class()
	case '.':
		p.pos++
		n := &node{k: kSet}
		for i := range n.set {
			n.set[i] = i != '\n'
		}
		return n
	case '*', '+', '?', '{':
		p.fail("%q has nothing to repeat", c)
	case '^', '$':
		p.fail("anchors are not supported: use name_full for a whole match")
	case '\\':
		p.pos++
		return &node{k: kSet, set: p.escape()}
	}
	p.pos++
	n := &node{k: kSet}
	n.set[c] = true
	return n
}

func hexval(c byte) int {
	switch {
	case c >= '0' && c <= '9':
		return int(c - '0')
	case c >= 'a' && c <= 'f':
		return int(c-'a') + 10
	case c >= 'A' && c <= 'F':
		return int(c-'A') + 10
	}
	return -1
}

func rangeSet(set *[256]bool, lo, hi int) {
	for i := lo; i <= hi; i++ {
		set[i] = true
	}
}

// the set after a backslash (p.pos is on the letter)
func (p *parser) escape() [256]bool {
	var s [256]bool
	if !p.more() {
		p.fail("a \\ at the end")
	}
	c := p.peek()
	p.pos++
	digit := func(s *[256]bool) { rangeSet(s, '0', '9') }
	word := func(s *[256]bool) { rangeSet(s, '0', '9'); rangeSet(s, 'a', 'z'); rangeSet(s, 'A', 'Z'); s['_'] = true }
	space := func(s *[256]bool) {
		s[' '], s['\t'], s['\n'], s['\r'], s['\f'], s['\v'] = true, true, true, true, true, true
	}
	negate := func(f func(*[256]bool)) [256]bool {
		var t, r [256]bool
		f(&t)
		for i := range r {
			r[i] = !t[i]
		}
		return r
	}
	switch c {
	case 'd':
		digit(&s)
	case 'w':
		word(&s)
	case 's':
		space(&s)
	case 'D':
		return negate(digit)
	case 'W':
		return negate(word)
	case 'S':
		return negate(space)
	case 'n':
		s['\n'] = true
	case 't':
		s['\t'] = true
	case 'r':
		s['\r'] = true
	case 'x':
		if p.pos+2 > len(p.s) || hexval(p.s[p.pos]) < 0 || hexval(p.s[p.pos+1]) < 0 {
			p.fail("\\x takes two hex digits")
		}
		s[hexval(p.s[p.pos])*16+hexval(p.s[p.pos+1])] = true
		p.pos += 2
	default:
		if (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') {
			p.pos--
			p.fail("unknown escape \\%c", c)
		}
		s[c] = true
	}
	return s
}

func (p *parser) class() *node {
	p.pos++ // [
	n := &node{k: kSet}
	neg := false
	if p.more() && p.peek() == '^' {
		neg = true
		p.pos++
	}
	first := true
	for {
		if !p.more() {
			p.fail("a [ is never closed")
		}
		c := p.peek()
		if c == ']' && !first {
			p.pos++
			break
		}
		first = false
		var lo int
		if c == '\\' {
			p.pos++
			es := p.escape()
			count, last := 0, 0
			for i, in := range es {
				if in {
					count++
					last = i
				}
			}
			if count != 1 {
				for i, in := range es {
					if in {
						n.set[i] = true
					}
				}
				continue
			}
			lo = last
		} else {
			lo = int(c)
			p.pos++
		}
		hi := lo
		if p.pos+1 < len(p.s) && p.peek() == '-' && p.s[p.pos+1] != ']' {
			p.pos++
			if p.peek() == '\\' {
				p.pos++
				es := p.escape()
				for i, in := range es {
					if in {
						hi = i
					}
				}
			} else {
				hi = int(p.peek())
				p.pos++
			}
			if hi < lo {
				p.fail("the range %c-%c runs backwards", rune(lo), rune(hi))
			}
		}
		rangeSet(&n.set, lo, hi)
	}
	if neg {
		for i := range n.set {
			n.set[i] = !n.set[i]
		}
	}
	return n
}

// ---- NFA (Thompson), then subset construction

type nfaState struct {
	set    *[256]bool // consumes one byte from set, then goes to out
	out    int
	eps    []int // empty moves, in priority order
	accept bool
	isTag  bool // an empty move that records the position in tag `tag`
	tag    int
}

type nfa struct{ st []nfaState }

func (m *nfa) add() int {
	m.st = append(m.st, nfaState{})
	return len(m.st) - 1
}

// build wires n in front of the given end state; returns its start
func (m *nfa) build(n *node, end int, tags bool) int {
	switch n.k {
	case kSet:
		s := m.add()
		m.st[s].set = &n.set
		m.st[s].out = end
		return s
	case kEmpty:
		return end
	case kGroup:
		if !tags {
			return m.build(n.subs[0], end, tags)
		}
		closeT := m.add()
		m.st[closeT].isTag, m.st[closeT].tag, m.st[closeT].eps = true, 2*(n.grp-1)+1, []int{end}
		inner := m.build(n.subs[0], closeT, tags)
		openT := m.add()
		m.st[openT].isTag, m.st[openT].tag, m.st[openT].eps = true, 2*(n.grp-1), []int{inner}
		return openT
	case kCat:
		next := end
		for i := len(n.subs) - 1; i >= 0; i-- {
			next = m.build(n.subs[i], next, tags)
		}
		return next
	case kAlt:
		s := m.add()
		for _, sub := range n.subs {
			m.st[s].eps = append(m.st[s].eps, m.build(sub, end, tags))
		}
		return s
	case kOpt:
		s := m.add()
		m.st[s].eps = []int{m.build(n.subs[0], end, tags), end}
		return s
	case kStar:
		s := m.add()
		body := m.build(n.subs[0], s, tags)
		m.st[s].eps = []int{body, end}
		return s
	case kPlus:
		s := m.add()
		body := m.build(n.subs[0], s, tags)
		m.st[s].eps = []int{body, end}
		return body
	}
	panic("unreachable")
}

func (m *nfa) closure(set []int) []int {
	seen := map[int]bool{}
	var out []int
	var visit func(int)
	visit = func(s int) {
		if seen[s] {
			return
		}
		seen[s] = true
		out = append(out, s)
		for _, e := range m.st[s].eps {
			visit(e)
		}
	}
	for _, s := range set {
		visit(s)
	}
	sort.Ints(out)
	return out
}

type dfa struct {
	next   [][256]int // -1: dead
	accept []bool
}

func key(set []int) string {
	var b strings.Builder
	for _, s := range set {
		fmt.Fprintf(&b, "%d,", s)
	}
	return b.String()
}

const maxStates = 400

func parse(pattern string) (tree *node, ngroups int, err error) {
	defer func() {
		if r := recover(); r != nil {
			if s, ok := r.(string); ok {
				err = fmt.Errorf("%s", s)
				return
			}
			panic(r)
		}
	}()
	p := &parser{s: pattern}
	tree = p.alt()
	if p.more() {
		p.fail("a ) without a (")
	}
	return tree, p.ngroups, nil
}

// the language DFA: a state is the set of NFA states reached, so it answers "does the whole text match?" (name_full)
func compile(pattern string) (d *dfa, err error) {
	defer func() {
		if r := recover(); r != nil {
			if s, ok := r.(string); ok {
				err = fmt.Errorf("%s", s)
				return
			}
			panic(r)
		}
	}()
	tree, _, err := parse(pattern)
	if err != nil {
		return nil, err
	}
	p := &parser{}
	m := &nfa{}
	end := m.add()
	m.st[end].accept = true
	start := m.build(tree, end, false)

	d = &dfa{}
	index := map[string]int{}
	var sets [][]int
	intern := func(set []int) int {
		k := key(set)
		if i, ok := index[k]; ok {
			return i
		}
		if len(sets) >= maxStates {
			p.fail("more than %d states: simplify the pattern or split it", maxStates)
		}
		index[k] = len(sets)
		sets = append(sets, set)
		d.next = append(d.next, [256]int{})
		acc := false
		for _, s := range set {
			if m.st[s].accept {
				acc = true
			}
		}
		d.accept = append(d.accept, acc)
		return len(sets) - 1
	}
	intern(m.closure([]int{start}))
	for i := 0; i < len(sets); i++ {
		for b := 0; b < 256; b++ {
			var to []int
			for _, s := range sets[i] {
				if m.st[s].set != nil && m.st[s].set[b] {
					to = append(to, m.st[s].out)
				}
			}
			if len(to) == 0 {
				d.next[i][b] = -1
			} else {
				d.next[i][b] = intern(m.closure(to))
			}
		}
	}
	return d, nil
}

// merge equivalent states (partition refinement) so the generated code is as small as the pattern allows
func (d *dfa) minimize() *dfa {
	n := len(d.next)
	class := make([]int, n)
	for i := range class {
		if d.accept[i] {
			class[i] = 1
		}
	}
	for {
		sig := map[string]int{}
		nc := make([]int, n)
		for i := 0; i < n; i++ {
			var b strings.Builder
			fmt.Fprintf(&b, "%d|", class[i])
			for c := 0; c < 256; c++ {
				t := d.next[i][c]
				if t >= 0 {
					t = class[t]
				}
				fmt.Fprintf(&b, "%d,", t)
			}
			k := b.String()
			if _, ok := sig[k]; !ok {
				sig[k] = len(sig)
			}
			nc[i] = sig[k]
		}
		done := len(sig) == distinct(class)
		class = nc
		if done {
			break
		}
	}
	// renumber so the start state stays 0
	rename := map[int]int{class[0]: 0}
	for i := 0; i < n; i++ {
		if _, ok := rename[class[i]]; !ok {
			rename[class[i]] = len(rename)
		}
	}
	out := &dfa{next: make([][256]int, len(rename)), accept: make([]bool, len(rename))}
	for i := 0; i < n; i++ {
		c := rename[class[i]]
		out.accept[c] = d.accept[i]
		for b := 0; b < 256; b++ {
			t := d.next[i][b]
			if t >= 0 {
				t = rename[class[t]]
			}
			out.next[c][b] = t
		}
	}
	return out
}

func distinct(a []int) int {
	m := map[int]bool{}
	for _, v := range a {
		m[v] = true
	}
	return len(m)
}

// ---- code generation

// the arms of `match c` for one state: bytes going to the same state are grouped into ranges
func arms(row [256]int) []string {
	byTarget := map[int][]string{}
	var order []int
	for b := 0; b < 256; {
		t := row[b]
		e := b
		for e+1 < 256 && row[e+1] == t {
			e++
		}
		if t >= 0 {
			if _, ok := byTarget[t]; !ok {
				order = append(order, t)
			}
			if e == b {
				byTarget[t] = append(byTarget[t], fmt.Sprint(b))
			} else {
				byTarget[t] = append(byTarget[t], fmt.Sprintf("%d..%d", b, e))
			}
		}
		b = e + 1
	}
	sort.Ints(order)
	var out []string
	for _, t := range order {
		out = append(out, fmt.Sprintf("%s => { st = %d; }", strings.Join(byTarget[t], ", "), t))
	}
	return out
}

func generate(out *strings.Builder, name, pat string) error {
	tree, ng, err := parse(pat)
	if err != nil {
		return err
	}
	lang, err := compile(pat)
	if err != nil {
		return err
	}
	plain, err := buildPrio(tree, ng, false)
	if err != nil {
		return err
	}
	fmt.Fprintf(out, "// ---- %s: %s\n", name, pat)
	emitAt(out, name, plain.minimize())
	emitFull(out, name, lang.minimize())
	emitFind(out, name)
	if ng > 0 {
		tagged, err := buildPrio(tree, ng, true)
		if err != nil {
			return err
		}
		emitCaps(out, name, ng, tagged)
	}
	out.WriteString("\n")
	return nil
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: regexgen patterns.rx > patterns.mito")
		os.Exit(2)
	}
	src, err := os.ReadFile(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	var out strings.Builder
	out.WriteString("// Generated by tools/regexgen from " + os.Args[1] + ". Do not edit: change the patterns and generate again.\n\n")
	bad := false
	for i, line := range strings.Split(strings.ReplaceAll(string(src), "\r\n", "\n"), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "//") {
			continue
		}
		f := strings.IndexAny(line, " \t")
		if f < 0 {
			fmt.Fprintf(os.Stderr, "%s:%d: expected `name pattern`\n", os.Args[1], i+1)
			bad = true
			continue
		}
		name, pat := line[:f], strings.TrimSpace(line[f:])
		if err := generate(&out, name, pat); err != nil {
			fmt.Fprintf(os.Stderr, "%s:%d: %s: %v\n    %s\n", os.Args[1], i+1, name, err, pat)
			bad = true
		}
	}
	if bad {
		os.Exit(1)
	}
	fmt.Print(out.String())
}
