package main

import (
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"net/url"
	"os"
	"strconv"
)

func torrentToMagnet(path string) (string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("reading file: %w", err)
	}

	infoBytes, topDict, err := extractInfo(data)
	if err != nil {
		return "", err
	}

	hash := sha1.Sum(infoBytes)
	magnet := "magnet:?xt=urn:btih:" + hex.EncodeToString(hash[:])

	if info, ok := topDict["info"].(map[string]interface{}); ok {
		if name, ok := info["name"].(string); ok && name != "" {
			magnet += "&dn=" + url.QueryEscape(name)
		}
	}

	seen := map[string]bool{}
	addTracker := func(s string) {
		if !seen[s] {
			seen[s] = true
			magnet += "&tr=" + url.QueryEscape(s)
		}
	}
	if ann, ok := topDict["announce"].(string); ok {
		addTracker(ann)
	}
	if al, ok := topDict["announce-list"].([]interface{}); ok {
		for _, tier := range al {
			if tl, ok := tier.([]interface{}); ok {
				for _, t := range tl {
					if s, ok := t.(string); ok {
						addTracker(s)
					}
				}
			}
		}
	}

	return magnet, nil
}

// extractInfo parses the top-level bencode dict and returns the raw bytes of
// the "info" value (for SHA1 hashing) plus the parsed dictionary.
func extractInfo(data []byte) ([]byte, map[string]interface{}, error) {
	if len(data) == 0 || data[0] != 'd' {
		return nil, nil, fmt.Errorf("invalid torrent: expected dict")
	}
	p := &bp{d: data, i: 1}
	dict := map[string]interface{}{}
	var infoRaw []byte

	for p.i < len(data) && data[p.i] != 'e' {
		key, err := p.str()
		if err != nil {
			return nil, nil, fmt.Errorf("key: %w", err)
		}
		start := p.i
		val, err := p.val()
		if err != nil {
			return nil, nil, fmt.Errorf("value for %q: %w", key, err)
		}
		if key == "info" {
			infoRaw = data[start:p.i]
		}
		dict[key] = val
	}

	if infoRaw == nil {
		return nil, nil, fmt.Errorf("no info dict in torrent file")
	}
	return infoRaw, dict, nil
}

// Minimal bencode parser that tracks byte positions.
type bp struct {
	d []byte
	i int
}

func (p *bp) val() (interface{}, error) {
	if p.i >= len(p.d) {
		return nil, fmt.Errorf("unexpected end at %d", p.i)
	}
	switch {
	case p.d[p.i] == 'i':
		return p.integer()
	case p.d[p.i] == 'l':
		return p.list()
	case p.d[p.i] == 'd':
		return p.dict()
	case p.d[p.i] >= '0' && p.d[p.i] <= '9':
		return p.str()
	default:
		return nil, fmt.Errorf("unexpected byte 0x%02x at %d", p.d[p.i], p.i)
	}
}

func (p *bp) str() (string, error) {
	start := p.i
	for p.i < len(p.d) && p.d[p.i] != ':' {
		p.i++
	}
	if p.i >= len(p.d) {
		return "", fmt.Errorf("unterminated string length at %d", start)
	}
	n, err := strconv.Atoi(string(p.d[start:p.i]))
	if err != nil {
		return "", err
	}
	p.i++
	if p.i+n > len(p.d) {
		return "", fmt.Errorf("string overflows at %d", p.i)
	}
	s := string(p.d[p.i : p.i+n])
	p.i += n
	return s, nil
}

func (p *bp) integer() (int64, error) {
	p.i++
	start := p.i
	for p.i < len(p.d) && p.d[p.i] != 'e' {
		p.i++
	}
	if p.i >= len(p.d) {
		return 0, fmt.Errorf("unterminated int at %d", start)
	}
	n, err := strconv.ParseInt(string(p.d[start:p.i]), 10, 64)
	p.i++
	return n, err
}

func (p *bp) list() ([]interface{}, error) {
	p.i++
	var out []interface{}
	for p.i < len(p.d) && p.d[p.i] != 'e' {
		v, err := p.val()
		if err != nil {
			return nil, err
		}
		out = append(out, v)
	}
	if p.i >= len(p.d) {
		return nil, fmt.Errorf("unterminated list")
	}
	p.i++
	return out, nil
}

func (p *bp) dict() (map[string]interface{}, error) {
	p.i++
	m := map[string]interface{}{}
	for p.i < len(p.d) && p.d[p.i] != 'e' {
		key, err := p.str()
		if err != nil {
			return nil, err
		}
		val, err := p.val()
		if err != nil {
			return nil, err
		}
		m[key] = val
	}
	if p.i >= len(p.d) {
		return nil, fmt.Errorf("unterminated dict")
	}
	p.i++
	return m, nil
}
