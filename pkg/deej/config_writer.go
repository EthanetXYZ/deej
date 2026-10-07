package deej

import (
	"bytes"
	"fmt"
	"os"
	"strings"

	"gopkg.in/yaml.v3"
)

// configEdit describes a single change to the user's config file.
// a nil Value removes the key (and any parent sections left empty by that removal)
type configEdit struct {
	Path  []string
	Value interface{}
}

// editConfigFile applies edits to the user's config.yaml in place. it works on the YAML node tree
// rather than re-serializing a map, so comments, key order and formatting are preserved
// as much as possible - the config file stays friendly to hand-editing
func editConfigFile(filepath string, edits ...configEdit) error {
	original, err := os.ReadFile(filepath)
	if err != nil {
		return fmt.Errorf("read config file: %w", err)
	}

	// the YAML library mangles CRLF inside comments, so work with LF and restore CRLF when writing if needed
	usesCRLF := bytes.Contains(original, []byte("\r\n"))
	original = bytes.ReplaceAll(original, []byte("\r\n"), []byte("\n"))

	var doc yaml.Node
	if err := yaml.Unmarshal(original, &doc); err != nil {
		return fmt.Errorf("parse config file: %w", err)
	}

	// an empty file has no document node yet
	if doc.Kind == 0 {
		doc = yaml.Node{Kind: yaml.DocumentNode, Content: []*yaml.Node{{Kind: yaml.MappingNode}}}
	}

	if doc.Kind != yaml.DocumentNode || len(doc.Content) == 0 || doc.Content[0].Kind != yaml.MappingNode {
		return fmt.Errorf("config file root isn't a YAML mapping")
	}

	root := doc.Content[0]

	for _, edit := range edits {
		if edit.Value == nil {
			removeNodePath(root, edit.Path)
			continue
		}

		valueNode := &yaml.Node{}
		if err := valueNode.Encode(edit.Value); err != nil {
			return fmt.Errorf("encode config value for %v: %w", edit.Path, err)
		}

		setNodePath(root, edit.Path, valueNode)
	}

	var buf bytes.Buffer
	encoder := yaml.NewEncoder(&buf)
	encoder.SetIndent(2)

	if err := encoder.Encode(&doc); err != nil {
		return fmt.Errorf("serialize config file: %w", err)
	}

	if err := encoder.Close(); err != nil {
		return fmt.Errorf("serialize config file: %w", err)
	}

	// write to a temp file first and then swap it in, so a crash mid-write can't leave a half-written config
	output := separateTopLevelSections(buf.Bytes())
	if usesCRLF {
		output = bytes.ReplaceAll(output, []byte("\n"), []byte("\r\n"))
	}

	tempPath := filepath + ".tmp"
	if err := os.WriteFile(tempPath, output, 0644); err != nil {
		return fmt.Errorf("write temp config file: %w", err)
	}

	if err := os.Rename(tempPath, filepath); err != nil {
		os.Remove(tempPath)
		return fmt.Errorf("replace config file: %w", err)
	}

	return nil
}

// findMappingValue returns the index (within mapping.Content) of the key node, or -1
func findMappingKey(mapping *yaml.Node, key string) int {
	for i := 0; i+1 < len(mapping.Content); i += 2 {
		if mapping.Content[i].Value == key {
			return i
		}
	}

	return -1
}

func setNodePath(mapping *yaml.Node, path []string, value *yaml.Node) {
	key := path[0]
	idx := findMappingKey(mapping, key)

	if len(path) == 1 {
		if idx >= 0 {

			// keep any comments that were attached to the old value
			old := mapping.Content[idx+1]
			value.LineComment = old.LineComment
			value.HeadComment = old.HeadComment
			value.FootComment = old.FootComment
			mapping.Content[idx+1] = value
		} else {
			mapping.Content = append(mapping.Content, newKeyNode(key), value)
		}

		return
	}

	var child *yaml.Node
	if idx >= 0 && mapping.Content[idx+1].Kind == yaml.MappingNode {
		child = mapping.Content[idx+1]
	} else {
		child = &yaml.Node{Kind: yaml.MappingNode}

		if idx >= 0 {
			mapping.Content[idx+1] = child
		} else {
			mapping.Content = append(mapping.Content, newKeyNode(key), child)
		}
	}

	setNodePath(child, path[1:], value)
}

// removeNodePath deletes the key at path, returning true if the mapping it was in became empty
func removeNodePath(mapping *yaml.Node, path []string) bool {
	idx := findMappingKey(mapping, path[0])
	if idx < 0 {
		return false
	}

	if len(path) > 1 {
		child := mapping.Content[idx+1]
		if child.Kind != yaml.MappingNode || !removeNodePath(child, path[1:]) {
			return false
		}
	}

	mapping.Content = append(mapping.Content[:idx], mapping.Content[idx+2:]...)

	return len(mapping.Content) == 0
}

// separateTopLevelSections puts a blank line before each top-level comment block and after nested sections,
// since the YAML encoder drops the blank lines that make deej's config easy to read
func separateTopLevelSections(content []byte) []byte {
	lines := strings.Split(string(content), "\n")
	result := make([]string, 0, len(lines)*2)

	for idx, line := range lines {
		topLevel := line != "" && line[0] != ' ' && line[0] != '-'

		if idx > 0 && topLevel {
			previous := lines[idx-1]
			previousIsComment := strings.HasPrefix(previous, "#")
			previousIsNested := strings.HasPrefix(previous, " ") || strings.HasPrefix(previous, "-")

			startsCommentBlock := strings.HasPrefix(line, "#") && !previousIsComment
			endsNestedSection := previousIsNested

			if previous != "" && (startsCommentBlock || endsNestedSection) {
				result = append(result, "")
			}
		}

		result = append(result, line)
	}

	return []byte(strings.Join(result, "\n"))
}

func newKeyNode(key string) *yaml.Node {
	node := &yaml.Node{Kind: yaml.ScalarNode, Value: key, Tag: "!!str"}

	// slider indexes look like ints in the hand-written config, so keep them that way
	var n int
	if _, err := fmt.Sscanf(key, "%d", &n); err == nil && fmt.Sprint(n) == key {
		node.Tag = "!!int"
	}

	return node
}
