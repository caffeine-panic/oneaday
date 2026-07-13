package main_test

import (
	"bufio"
	"encoding/json"
	"os/exec"
	"reflect"
	"testing"
)

type response struct {
	JSONRPC string `json:"jsonrpc"`
	ID      string `json:"id"`
	Result  struct {
		ProtocolVersion string `json:"protocolVersion"`
		Adapters        []struct {
			ID     string `json:"id"`
			Status string `json:"status"`
		} `json:"adapters"`
	} `json:"result"`
}

func TestSidecarReportsRegistryCapabilities(t *testing.T) {
	command := exec.Command("go", "run", ".")
	stdin, err := command.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	stdout, err := command.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = command.Process.Kill() })

	_, err = stdin.Write([]byte("{\"jsonrpc\":\"2.0\",\"id\":\"capabilities-1\",\"method\":\"system.capabilities\"}\n"))
	if err != nil {
		t.Fatal(err)
	}

	line, err := bufio.NewReader(stdout).ReadBytes('\n')
	if err != nil {
		t.Fatal(err)
	}
	var got response
	if err := json.Unmarshal(line, &got); err != nil {
		t.Fatalf("decode response: %v; output: %s", err, line)
	}

	if got.JSONRPC != "2.0" || got.ID != "capabilities-1" {
		t.Fatalf("response envelope = %#v", got)
	}
	if got.Result.ProtocolVersion != "0.1.0" {
		t.Fatalf("protocol version = %q", got.Result.ProtocolVersion)
	}
	wantAdapters := []struct {
		ID     string `json:"id"`
		Status string `json:"status"`
	}{{"etcd", "planned"}, {"zookeeper", "planned"}, {"nacos", "planned"}}
	if !reflect.DeepEqual(got.Result.Adapters, wantAdapters) {
		t.Fatalf("adapters = %#v, want %#v", got.Result.Adapters, wantAdapters)
	}
}
