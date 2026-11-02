package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"time"
)

// KubeClient habla con la API de Kubernetes desde dentro de un pod, con el token y el certificado
// que Kubernetes monta para el ServiceAccount. Son dos llamadas (GET y PATCH): un cliente mínimo
// mantiene el foco en Temporal, sin la dependencia de client-go.
type KubeClient struct {
	baseURL string
	token   string
	http    *http.Client
}

const serviceAccountDir = "/var/run/secrets/kubernetes.io/serviceaccount"

func NewInClusterKubeClient() (*KubeClient, error) {
	host, port := os.Getenv("KUBERNETES_SERVICE_HOST"), os.Getenv("KUBERNETES_SERVICE_PORT")
	if host == "" || port == "" {
		return nil, fmt.Errorf("not running inside Kubernetes (KUBERNETES_SERVICE_HOST is empty)")
	}
	token, err := os.ReadFile(serviceAccountDir + "/token")
	if err != nil {
		return nil, fmt.Errorf("reading service account token: %w", err)
	}
	caPEM, err := os.ReadFile(serviceAccountDir + "/ca.crt")
	if err != nil {
		return nil, fmt.Errorf("reading cluster CA: %w", err)
	}
	pool := x509.NewCertPool()
	pool.AppendCertsFromPEM(caPEM)
	return &KubeClient{
		baseURL: "https://" + host + ":" + port,
		token:   strings.TrimSpace(string(token)),
		http: &http.Client{
			Timeout:   10 * time.Second,
			Transport: &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}},
		},
	}, nil
}

func (k *KubeClient) Get(ctx context.Context, path string, out any) error {
	body, err := k.do(ctx, http.MethodGet, path, "", nil)
	if err != nil {
		return err
	}
	return json.Unmarshal(body, out)
}

func (k *KubeClient) MergePatch(ctx context.Context, path, patch string) error {
	_, err := k.do(ctx, http.MethodPatch, path, "application/merge-patch+json", strings.NewReader(patch))
	return err
}

func (k *KubeClient) do(ctx context.Context, method, path, contentType string, body io.Reader) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, method, k.baseURL+path, body)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+k.token)
	if contentType != "" {
		req.Header.Set("Content-Type", contentType)
	}
	res, err := k.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	data, err := io.ReadAll(res.Body)
	if err != nil {
		return nil, err
	}
	if res.StatusCode >= 300 {
		// Un 403 aquí significa que falta un permiso en src/k8s/temporal/rbac.yaml.
		return nil, fmt.Errorf("%s %s: %s: %s", method, path, res.Status, strings.TrimSpace(string(data)))
	}
	return data, nil
}
