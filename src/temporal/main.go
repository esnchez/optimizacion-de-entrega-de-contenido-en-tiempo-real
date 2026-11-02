// Worker de Temporal: se conecta al servidor, escucha la cola "operations" y ejecuta los workflows
// y activities registrados. Expone sus métricas para Prometheus en :9090/metrics.
package main

import (
	"log/slog"
	"os"
	"time"

	prom "github.com/prometheus/client_golang/prometheus"
	"github.com/uber-go/tally/v4"
	"github.com/uber-go/tally/v4/prometheus"
	"go.temporal.io/sdk/client"
	sdktally "go.temporal.io/sdk/contrib/tally"
	"go.temporal.io/sdk/log"
	"go.temporal.io/sdk/worker"
)

func env(name, def string) string {
	if v := os.Getenv(name); v != "" {
		return v
	}
	return def
}

// newMetricsScope publica las métricas del SDK (workflows completados y fallidos, activities,
// reintentos, latencias, huecos libres del worker) en formato Prometheus.
func newMetricsScope(listenAddr string, logger *slog.Logger) tally.Scope {
	reporter, err := prometheus.Configuration{
		ListenAddress: listenAddr,
		TimerType:     "histogram",
	}.NewReporter(prometheus.ConfigurationOptions{
		Registry: prom.NewRegistry(),
		OnError:  func(err error) { logger.Error("metrics reporter", "error", err) },
	})
	if err != nil {
		logger.Error("creating metrics reporter", "error", err)
		os.Exit(1)
	}
	scope, _ := tally.NewRootScope(tally.ScopeOptions{
		CachedReporter:  reporter,
		Separator:       prometheus.DefaultSeparator,
		SanitizeOptions: &sdktally.PrometheusSanitizeOptions,
	}, time.Second)
	return sdktally.NewPrometheusNamingScope(scope)
}

func main() {
	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	address := env("TEMPORAL_ADDRESS", "localhost:7233")

	kube, err := NewInClusterKubeClient()
	if err != nil {
		logger.Error("kubernetes client", "error", err)
		os.Exit(1)
	}

	options := client.Options{
		HostPort:       address,
		Namespace:      env("TEMPORAL_NAMESPACE", "default"),
		Logger:         log.NewStructuredLogger(logger),
		MetricsHandler: sdktally.NewMetricsHandler(newMetricsScope(env("METRICS_ADDR", ":9090"), logger)),
	}
	// Al desplegar todo a la vez, el worker suele arrancar antes de que el servidor acepte
	// conexiones. Reintentar aquí evita reinicios del pod (y falsas alertas de reinicio).
	var c client.Client
	for attempt, delay := 1, time.Second; ; attempt++ {
		var err error
		if c, err = client.Dial(options); err == nil {
			break
		}
		if attempt == 10 {
			logger.Error("connecting to Temporal", "address", address, "error", err)
			os.Exit(1)
		}
		logger.Warn("Temporal not reachable yet, retrying", "address", address, "attempt", attempt, "in", delay.String())
		time.Sleep(delay)
		delay = min(delay*2, 15*time.Second)
	}
	defer c.Close()

	w := worker.New(c, TaskQueue, worker.Options{})
	w.RegisterWorkflow(PrescaleForEvent)
	w.RegisterActivity(&Activities{Kube: kube})

	logger.Info("worker started", "address", address, "taskQueue", TaskQueue)
	// worker.InterruptCh: termina limpiamente con SIGTERM (lo que envía Kubernetes al parar el pod).
	if err := w.Run(worker.InterruptCh()); err != nil {
		logger.Error("worker stopped", "error", err)
		os.Exit(1)
	}
}
