package main

import (
	"math/rand"
	"regexp"
	"testing"
)

// longest match end from p, as the generated code computes it
func (d *dfa) longest(s string, p int) int {
	st, last := 0, -1
	if d.accept[0] {
		last = p
	}
	for i := p; i < len(s); i++ {
		st = d.next[st][s[i]]
		if st < 0 {
			break
		}
		if d.accept[st] {
			last = i + 1
		}
	}
	return last
}

// every DFA (minimized or not) agrees with Go's regexp, in its leftmost-longest mode, on random strings over the pattern's own alphabet
func TestAgainstRegexp(t *testing.T) {
	patterns := []string{
		`[a-z0-9]+(-[a-z0-9]+)*`,
		`[a-z0-9._-]+@[a-z0-9-]+(\.[a-z0-9-]+)+`,
		`-?\d+(\.\d+)?`,
		`\d{4}-\d{2}-\d{2}`,
		`[A-Za-z_]\w*`,
		`a|ab|abc`,
		`(a|b)*abb`,
		`(ab|a)(bc|c)?`,
		`x?`,
		`(a*)*b`,
		`[^"]*"`,
		`a{2,4}`,
		`a{2,}b`,
		`(foo|foobar)(bar)?`,
	}
	alphabet := "ab-c.@x0129\"foobar _AZ"
	rng := rand.New(rand.NewSource(1))
	for _, pat := range patterns {
		d, err := compile(pat)
		if err != nil {
			t.Fatalf("%s: %v", pat, err)
		}
		m := d.minimize()
		if len(m.next) > len(d.next) {
			t.Errorf("%s: minimizing grew the DFA", pat)
		}
		re := regexp.MustCompile(`^(?:` + pat + `)`)
		re.Longest()
		for n := 0; n < 3000; n++ {
			b := make([]byte, rng.Intn(14))
			for i := range b {
				b[i] = alphabet[rng.Intn(len(alphabet))]
			}
			s := string(b)
			want := -1
			if loc := re.FindStringIndex(s); loc != nil {
				want = loc[1]
			}
			if got := d.longest(s, 0); got != want {
				t.Fatalf("%s on %q: dfa %d, regexp %d", pat, s, got, want)
			}
			if got := m.longest(s, 0); got != want {
				t.Fatalf("%s on %q: minimized %d, regexp %d", pat, s, got, want)
			}
		}
	}
}

func TestErrors(t *testing.T) {
	for _, pat := range []string{`(ab`, `ab)`, `*a`, `[a-`, `[z-a]`, `^a`, `a$`, `\q`, `a{3,1}`, `a{`, `\x4`} {
		if _, err := compile(pat); err == nil {
			t.Errorf("%q should be refused", pat)
		}
	}
}

// what the generated name_match does, in Go: the end of the match and each group's start and end (-1 for unset)
func (d *pdfa) run(s string, ngroups int) (int, []int) {
	reg := make([][]int, d.slots)
	for j := range reg {
		reg[j] = make([]int, d.nTags)
		for t := range reg[j] {
			reg[j][t] = -1
		}
	}
	best := make([]int, d.nTags)
	for t := range best {
		best[t] = -1
	}
	end := -1
	record := func(slot, pos int) {
		end = pos
		copy(best, reg[slot])
	}
	for j, mv := range d.initial {
		for _, t := range mv.tags {
			reg[j][t] = 0
		}
	}
	st := 0
	if d.accept[0] {
		record(d.accSlot[0], 0)
	}
	for i := 0; i < len(s); i++ {
		nx := d.next[st][s[i]]
		if nx < 0 {
			break
		}
		mv := d.moves[st][s[i]]
		nr := make([][]int, len(mv))
		for j, m := range mv {
			nr[j] = append([]int{}, reg[m.from]...)
			for _, t := range m.tags {
				nr[j][t] = i + 1
			}
		}
		for j := range nr {
			copy(reg[j], nr[j])
		}
		st = nx
		if d.accept[st] {
			record(d.accSlot[st], i+1)
		}
	}
	return end, best
}

func TestCapturesAgainstRegexp(t *testing.T) {
	patterns := []string{
		`(\d{4})-(\d{2})-(\d{2})`,
		`(a|b)*c`,
		`(x)?y`,
		`((a)b)+`,
		`(a|ab)(c|bcd)`,
		`(a){2,3}`,
		`(a+)(b+)?`,
		`(?:(a)|b)*c`,
		`([a-z]+)@([a-z]+)\.(com|org)`,
		`(a*)(a*)`,
		`(a|ab)(bc|c)?`,
		`(foo|foobar)(bar)?`,
		`a(b|c)*d`,
		`(\w+)=(\w*)`,
		`((a|b)(c|d))*e`,
		`(a?)(ab)?`,
	}
	alphabet := "abcdxyz=@.-0123 "
	rng := rand.New(rand.NewSource(2))
	for _, pat := range patterns {
		tree, ng, err := parse(pat)
		if err != nil {
			t.Fatalf("%s: %v", pat, err)
		}
		d, err := buildPrio(tree, ng, true)
		if err != nil {
			t.Fatalf("%s: %v", pat, err)
		}
		plain, err := buildPrio(tree, ng, false)
		if err != nil {
			t.Fatal(err)
		}
		pm := plain.minimize()
		re := regexp.MustCompile(`^(?:` + pat + `)`)
		for n := 0; n < 4000; n++ {
			b := make([]byte, rng.Intn(12))
			for i := range b {
				b[i] = alphabet[rng.Intn(len(alphabet))]
			}
			s := string(b)
			want := re.FindStringSubmatchIndex(s)
			end, caps := d.run(s, ng)
			if want == nil {
				if end >= 0 {
					t.Fatalf("%s on %q: matched to %d, regexp: no match", pat, s, end)
				}
				continue
			}
			if end != want[1] {
				t.Fatalf("%s on %q: ends at %d, regexp %d", pat, s, end, want[1])
			}
			for g := 1; g <= ng; g++ {
				if caps[2*(g-1)] != want[2*g] || caps[2*(g-1)+1] != want[2*g+1] {
					t.Fatalf("%s on %q: group %d is %d..%d, regexp %d..%d", pat, s, g, caps[2*(g-1)], caps[2*(g-1)+1], want[2*g], want[2*g+1])
				}
			}
			// the matcher without captures (minimized) agrees on the end
			if e2, _ := (&pdfa{next: pm.next, accept: pm.accept, accSlot: accSlots(pm.accept), moves: nil}).endOnly(s); e2 != want[1] {
				t.Fatalf("%s on %q: plain matcher ends at %d, regexp %d", pat, s, e2, want[1])
			}
		}
	}
}

func accSlots(acc []bool) []int {
	r := make([]int, len(acc))
	for i, a := range acc {
		if a {
			r[i] = 0
		} else {
			r[i] = -1
		}
	}
	return r
}

func (d *pdfa) endOnly(s string) (int, bool) {
	st, last := 0, -1
	if d.accept[0] {
		last = 0
	}
	for i := 0; i < len(s); i++ {
		st = d.next[st][s[i]]
		if st < 0 {
			break
		}
		if d.accept[st] {
			last = i + 1
		}
	}
	return last, last >= 0
}
