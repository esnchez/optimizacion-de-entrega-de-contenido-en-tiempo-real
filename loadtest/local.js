// Prueba de carga local (escala de una máquina) contra el servicio de entrega de contenido.
// Mezcla de tráfico realista: objetos pequeños cacheados, revalidación (304), HEAD y Range
// sobre un objeto grande (como un reproductor de vídeo haciendo seek).
//
// Ejecución en Docker (entorno de Compose de src/tests/compose.yaml, con `task env:seed`):
//   docker run --rm -i --network content-delivery-e2e_default -e BASE_URL=http://app:8080 grafana/k6 run - < loadtest/local.js
// Ejecución en Kubernetes con métricas a Prometheus: ver loadtest/k8s-job.yaml y docs/operaciones/monitorizacion.md.

import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const VUS = parseInt(__ENV.VUS || '200', 10);
const DURATION = __ENV.DURATION || '30s';
// Fracción de peticiones Range (van siempre a S3). RANGE_SHARE=0 mide solo el servicio y sus cachés.
const RANGE_SHARE = parseFloat(__ENV.RANGE_SHARE || '0.2');
// Cabecera Host opcional: para atacar el Ingress por su Service interno (host cdn.local).
const HOST = __ENV.HOST_HEADER;
const withHost = (headers = {}) => (HOST ? { ...headers, Host: HOST } : headers);

// PROFILE=steady (por defecto): VUS usuarios sin pausa durante DURATION (prueba de saturación).
// PROFILE=scaling: tasa de llegada controlada (peticiones/s) que sube, se mantiene y baja, para
// observar en Grafana cómo escala el HPA y cómo vuelve a reducir. Con tasa fija, la carga no
// baja aunque el sistema se ralentice (modelo abierto, como el tráfico real).
const PROFILE = __ENV.PROFILE || 'steady';
const PEAK_RPS = parseInt(__ENV.PEAK_RPS || '1500', 10);
const BASE_RPS = parseInt(__ENV.BASE_RPS || '100', 10);

// PROFILE=connections: conexiones simultáneas, como espectadores de vídeo. Cada VU es un
// espectador con UNA conexión keep-alive: pide un segmento y espera THINK_TIME segundos (lo que
// dura el segmento) antes del siguiente. Muchas conexiones abiertas, pocas peticiones por segundo.
// La carga sube en STEPS escalones hasta CONNECTIONS y se mantiene STEP_HOLD en cada uno: cada
// escalón da un punto (conexiones -> memoria, CPU, latencia) para extrapolar la capacidad por pod.
const CONNECTIONS = parseInt(__ENV.CONNECTIONS || '2000', 10);
const STEPS = parseInt(__ENV.STEPS || '4', 10);
const STEP_RAMP = __ENV.STEP_RAMP || '1m';
const STEP_HOLD = __ENV.STEP_HOLD || '3m';
const THINK_TIME = parseFloat(__ENV.THINK_TIME || '4');
const SEGMENT_KEY = __ENV.SEGMENT_KEY || 'videos/segment.bin';

const connectionStages = [];
for (let i = 1; i <= STEPS; i++) {
  const target = Math.round((CONNECTIONS * i) / STEPS);
  connectionStages.push({ duration: STEP_RAMP, target }, { duration: STEP_HOLD, target });
}
connectionStages.push({ duration: '1m', target: 0 });

const scenarios = {
  steady: {
    executor: 'ramping-vus',
    startVUs: 0,
    stages: [
      { duration: '5s', target: VUS },
      { duration: DURATION, target: VUS },
      { duration: '3s', target: 0 },
    ],
  },
  scaling: {
    executor: 'ramping-arrival-rate',
    startRate: BASE_RPS,
    timeUnit: '1s',
    preAllocatedVUs: 50,
    maxVUs: 400,
    stages: [
      { duration: '2m', target: BASE_RPS }, // línea base: 2 pods, CPU baja
      { duration: '2m', target: PEAK_RPS }, // rampa: el HPA debería escalar
      { duration: '4m', target: PEAK_RPS }, // pico sostenido: la carga se reparte entre más pods
      { duration: '1m', target: BASE_RPS }, // bajada
      { duration: '7m', target: BASE_RPS }, // 5 min de estabilización y luego reducción gradual
    ],
  },
  connections: {
    executor: 'ramping-vus',
    startVUs: 0,
    stages: connectionStages,
    gracefulRampDown: '10s',
  },
};

export const options = {
  scenarios: { [PROFILE]: scenarios[PROFILE] },
  // Con miles de VUs, guardar cada cuerpo en memoria limitaría cuántas conexiones caben en k6.
  discardResponseBodies: PROFILE === 'connections',
  thresholds: {
    // Objetivos del README: p99 < 50ms y tasa de error < 0.1%.
    http_req_duration: ['p(95)<25', 'p(99)<50'],
    http_req_failed: ['rate<0.001'],
    // Desglose por tipo de petición. Los Range van siempre a S3: con LocalStack (Python) su
    // latencia mide sobre todo a LocalStack, no al servicio.
    'http_req_duration{kind:small}': ['p(99)<50'],
    'http_req_duration{kind:revalidate}': ['p(99)<50'],
    'http_req_duration{kind:head}': ['p(99)<50'],
    'http_req_duration{kind:range}': ['p(99)<500'],
    'http_req_duration{kind:segment}': ['p(99)<50'],
  },
};

let etag = null;

function viewer() {
  // Primer segmento en un instante aleatorio: sin esto, los VUs que arrancan juntos piden a la
  // vez cada THINK_TIME segundos, en oleadas que no se parecen al tráfico real.
  if (__ITER === 0) sleep(Math.random() * THINK_TIME);
  const res = http.get(`${BASE_URL}/content/${SEGMENT_KEY}`, {
    headers: withHost(),
    tags: { kind: 'segment' },
  });
  check(res, { 'segment 200': (x) => x.status === 200 });
  sleep(THINK_TIME);
}

export default function () {
  if (PROFILE === 'connections') {
    viewer();
    return;
  }
  let res;
  if (Math.random() < RANGE_SHARE) {
    const start = Math.floor(Math.random() * 3000000);
    res = http.get(`${BASE_URL}/content/videos/big.bin`, {
      headers: withHost({ Range: `bytes=${start}-${start + 65535}` }),
      responseType: 'binary', // cuerpo como bytes: decodificarlo como texto altera su longitud
      tags: { kind: 'range' },
    });
    check(res, { 'range 206': (x) => x.status === 206 && x.body.byteLength === 65536 });
    return;
  }
  const r = Math.random();
  if (r < 0.6) {
    res = http.get(`${BASE_URL}/content/hello.txt`, { headers: withHost(), tags: { kind: 'small' } });
    etag = res.headers.Etag || etag;
    check(res, { 'small 200': (x) => x.status === 200 && x.body === 'hello world' });
  } else if (r < 0.85) {
    res = http.get(`${BASE_URL}/content/hello.txt`, {
      headers: withHost(etag ? { 'If-None-Match': etag } : {}),
      tags: { kind: 'revalidate' },
    });
    check(res, { 'revalidate 304/200': (x) => x.status === 304 || x.status === 200 });
  } else {
    res = http.head(`${BASE_URL}/content/videos/big.bin`, { headers: withHost(), tags: { kind: 'head' } });
    check(res, { 'head 200': (x) => x.status === 200 });
  }
}
