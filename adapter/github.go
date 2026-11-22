package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/actions/scaleset"
	"io"
	"net/http"
	"os"
	"strings"
)

type githubConfig struct {
	RegistrationURL string   `json:"registration_url"`
	ScaleSetName    string   `json:"scale_set_name"`
	Labels          []string `json:"labels"`
	RunnerGroup     string   `json:"runner_group"`
	MaxRunners      *int     `json:"max_runners"`
	Auth            struct {
		GitHubApp *struct {
			ClientID       string `json:"client_id"`
			InstallationID int64  `json:"installation_id"`
			PrivateKeyFile string `json:"private_key_file"`
		} `json:"github_app"`
		TokenFile string `json:"token_file"`
	} `json:"auth"`
}

func loadConfig(path string) (githubConfig, error) {
	var c githubConfig
	data, err := os.ReadFile(path)
	if err != nil {
		return c, err
	}
	if err = json.Unmarshal(data, &c); err != nil {
		return c, err
	}
	if c.RegistrationURL == "" {
		return c, errors.New("missing registration_url")
	}
	if c.ScaleSetName == "" {
		return c, errors.New("missing scale_set_name")
	}
	if c.Labels == nil {
		return c, errors.New("missing labels")
	}
	for _, label := range c.Labels {
		if strings.TrimSpace(label) == "" {
			return c, errors.New("empty labels entry")
		}
	}
	if c.MaxRunners == nil || *c.MaxRunners < 0 {
		return c, errors.New("missing or invalid max_runners")
	}
	if c.RunnerGroup == "" {
		c.RunnerGroup = "Default"
	}
	if c.Auth.GitHubApp != nil && c.Auth.TokenFile != "" {
		return c, errors.New("auth must contain either github_app or token_file")
	}
	if a := c.Auth.GitHubApp; a != nil {
		if a.ClientID == "" {
			return c, errors.New("missing auth.github_app.client_id")
		}
		if a.InstallationID == 0 {
			return c, errors.New("missing auth.github_app.installation_id")
		}
		if a.PrivateKeyFile == "" {
			return c, errors.New("missing auth.github_app.private_key_file")
		}
	} else if c.Auth.TokenFile == "" {
		return c, errors.New("missing auth.token_file or auth.github_app")
	}
	return c, nil
}
func readSecret(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil {
		return "", err
	}
	if info.Mode().Perm()&077 != 0 {
		return "", fmt.Errorf("%s: chmod 600 required", path)
	}
	data, err := io.ReadAll(f)
	if err != nil {
		return "", err
	}
	secret := strings.TrimSpace(string(data))
	if secret == "" {
		return "", fmt.Errorf("%s: secret is empty", path)
	}
	return secret, nil
}
func systemInfo(id int) scaleset.SystemInfo {
	return scaleset.SystemInfo{System: "smolrunners", Subsystem: "adapter", Version: "1", ScaleSetID: id}
}
func (c githubConfig) client() (*scaleset.Client, error) {
	if a := c.Auth.GitHubApp; a != nil {
		key, err := readSecret(a.PrivateKeyFile)
		if err != nil {
			return nil, fmt.Errorf("private_key_file: %w", err)
		}
		return scaleset.NewClientWithGitHubApp(scaleset.ClientWithGitHubAppConfig{GitHubConfigURL: c.RegistrationURL, GitHubAppAuth: scaleset.GitHubAppAuth{ClientID: a.ClientID, InstallationID: a.InstallationID, PrivateKey: key}, SystemInfo: systemInfo(0)})
	}
	token, err := readSecret(c.Auth.TokenFile)
	if err != nil {
		return nil, fmt.Errorf("token_file: %w", err)
	}
	return scaleset.NewClientWithPersonalAccessToken(scaleset.NewClientWithPersonalAccessTokenConfig{GitHubConfigURL: c.RegistrationURL, PersonalAccessToken: token, SystemInfo: systemInfo(0)})
}
func (c githubConfig) setup(ctx context.Context) (*scaleset.Client, *scaleset.MessageSessionClient, int, error) {
	client, err := c.client()
	if err != nil {
		return nil, nil, 0, err
	}
	groupID := 1
	if !strings.EqualFold(c.RunnerGroup, "Default") {
		group, e := client.GetRunnerGroupByName(ctx, c.RunnerGroup)
		if e != nil {
			return nil, nil, 0, e
		}
		if group == nil {
			return nil, nil, 0, fmt.Errorf("runner_group %q not found", c.RunnerGroup)
		}
		groupID = group.ID
	}
	set, err := client.GetRunnerScaleSet(ctx, groupID, c.ScaleSetName)
	if err != nil {
		return nil, nil, 0, err
	}
	if set == nil {
		labels := make([]scaleset.Label, 0, len(c.Labels))
		for _, name := range c.Labels {
			labels = append(labels, scaleset.Label{Name: name})
		}
		set, err = client.CreateRunnerScaleSet(ctx, &scaleset.RunnerScaleSet{Name: c.ScaleSetName, RunnerGroupID: groupID, Labels: labels, RunnerSetting: scaleset.RunnerSetting{DisableUpdate: true}})
		if err != nil {
			return nil, nil, 0, err
		}
	}
	client.SetSystemInfo(systemInfo(set.ID))
	host, err := os.Hostname()
	if err != nil {
		return nil, nil, 0, err
	}
	session, err := client.MessageSessionClient(ctx, set.ID, host)
	if err != nil {
		return nil, nil, 0, err
	}
	return client, session, set.ID, nil
}
func notFound(err error) bool {
	var response *scaleset.RequestResponseError
	return errors.As(err, &response) && response.Response != nil && response.Response.StatusCode == http.StatusNotFound
}
