import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Divider } from "primereact/divider";
import { Message } from "primereact/message";
import { TabPanel, TabView } from "primereact/tabview";
import { Tag } from "primereact/tag";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useRef, useState } from "react";
import { getConfig, getNetInfo, getPac } from "../api/client";
import type { Config, PacInfo } from "../api/types";

function Code({ children }: { children: string }) {
  return (
    <code
      style={{
        display: "block",
        padding: "0.65rem 0.85rem",
        background: "var(--surface-ground)",
        borderRadius: "var(--border-radius)",
        fontSize: "0.9rem",
        whiteSpace: "pre-wrap",
        wordBreak: "break-all",
        margin: "0.35rem 0",
      }}
    >
      {children}
    </code>
  );
}

function Step({ n, children }: { n: number; children: React.ReactNode }) {
  return (
    <div className="flex gap-2 mb-2">
      <Tag value={String(n)} severity="info" style={{ minWidth: "2rem", height: "2rem" }} />
      <div style={{ flex: 1, lineHeight: "1.65" }}>{children}</div>
    </div>
  );
}

function IpCard({
  label,
  ip,
  selected,
  onSelect,
}: {
  label: string;
  ip: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      className="stat-card"
      onClick={onSelect}
      style={{
        cursor: "pointer",
        textAlign: "left",
        font: "inherit",
        outline: selected ? "2px solid var(--primary-color)" : undefined,
        background: selected ? "var(--primary-50)" : undefined,
      }}
    >
      <div className="label">{label}</div>
      <div className={selected ? "value accent" : "value"} style={{ fontSize: "1.25rem" }}>
        {ip}
      </div>
    </button>
  );
}

async function detectExternalIp(): Promise<string | null> {
  for (const url of ["https://ifconfig.me/ip", "https://api.ipify.org"]) {
    try {
      const r = await fetch(url, { signal: AbortSignal.timeout(4000) });
      if (r.ok) {
        const t = (await r.text()).trim();
        if (/^\d{1,3}(\.\d{1,3}){3}$/.test(t) || t.includes(":")) return t;
      }
    } catch {
      // try next
    }
  }
  return null;
}

export default function Connect() {
  const [cfg, setCfg] = useState<Config | null>(null);
  const [localIps, setLocalIps] = useState<string[]>(["127.0.0.1"]);
  const [externalIp, setExternalIp] = useState<string | null>(null);
  const [selectedIp, setSelectedIp] = useState<string>("127.0.0.1");
  const [pac, setPac] = useState<PacInfo | null>(null);
  const toast = useRef<Toast>(null);

  const refresh = useCallback(async () => {
    try {
      const [c, net, p] = await Promise.all([
        getConfig(),
        getNetInfo(),
        getPac().catch(() => null),
      ]);
      setCfg(c);
      setPac(p);
      const ips = net.local_ips.length > 0 ? net.local_ips : ["127.0.0.1"];
      setLocalIps(ips);
      setSelectedIp((prev) => (ips.includes(prev) ? prev : ips[0]));
    } catch {
      setLocalIps(["127.0.0.1"]);
      setSelectedIp("127.0.0.1");
    }
    const ext = await detectExternalIp();
    setExternalIp(ext);
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const copy = (text: string) => {
    navigator.clipboard
      .writeText(text)
      .then(() =>
        toast.current?.show({
          severity: "success",
          summary: "Скопировано",
          detail: text,
          life: 1500,
        }),
      )
      .catch(() => {
        /* clipboard может быть недоступен */
      });
  };

  const httpPort = cfg?.http.port ?? 3128;
  const httpsPort = cfg?.https.port ?? 3129;
  const socksPort = cfg?.socks5.port ?? 1080;
  const ip = selectedIp;
  const pacPort = pac?.port ?? 8081;
  const pacUrl = `http://${ip}:${pacPort}/proxy.pac`;
  const pacModeLabel =
    pac?.mode === "http"
      ? "только HTTP-прокси"
      : pac?.mode === "socks"
        ? "только SOCKS5"
        : "HTTP-прокси + SOCKS5";

  return (
    <>
      <Toast ref={toast} />
      <h1 className="page-title">Подключение</h1>
      <Message
        severity="info"
        text="Прокси слушает на 0.0.0.0 — подключайтесь с других устройств по LAN-адресу. API и эта панель доступны только на 127.0.0.1. Кликните по карточке IP — этот адрес будет использован в инструкциях ниже."
        className="mb-3 w-full"
      />

      <h2 style={{ fontSize: "1.1rem", margin: "0 0 0.75rem" }}>Выберите адрес подключения</h2>
      <div className="stat-grid">
        {localIps.map((lip) => (
          <IpCard
            key={lip}
            label={lip.startsWith("172.") ? "Docker / bridge" : "LAN-адрес"}
            ip={lip}
            selected={selectedIp === lip}
            onSelect={() => setSelectedIp(lip)}
          />
        ))}
        <IpCard
          label="Loopback (этот компьютер)"
          ip="127.0.0.1"
          selected={selectedIp === "127.0.0.1"}
          onSelect={() => setSelectedIp("127.0.0.1")}
        />
        {externalIp && (
          <IpCard
            label="Внешний (ifconfig.me)"
            ip={externalIp}
            selected={selectedIp === externalIp}
            onSelect={() => setSelectedIp(externalIp)}
          />
        )}
      </div>

      <div className="flex gap-2 mt-2">
        <Button label="Обновить IP" icon="pi pi-refresh" text onClick={refresh} />
        <Button label="Скопировать выбранный" icon="pi pi-copy" text onClick={() => copy(ip)} />
      </div>

      <Card title={`Адрес для подключения: ${ip}`} className="mt-3">
        <p style={{ marginTop: 0 }}>
          HTTP-прокси: <Tag value={`${ip}:${httpPort}`} severity="info" /> · HTTPS (MITM):{" "}
          <Tag value={`${ip}:${httpsPort}`} severity="info" /> · SOCKS5:{" "}
          <Tag value={`${ip}:${socksPort}`} severity="secondary" />
        </p>
      </Card>

      {pac?.enabled !== false && (
        <Card title="PAC / WPAD — автоматическая настройка прокси" className="mt-3">
          <p style={{ marginTop: 0 }}>
            URL PAC-скрипта для выбранного адреса (режим:{" "}
            <Tag value={pacModeLabel} severity="info" />):
          </p>
          <Code>{pacUrl}</Code>
          <div className="flex gap-2 mb-3">
            <Button
              label="Скопировать URL"
              icon="pi pi-copy"
              text
              onClick={() => copy(pacUrl)}
            />
          </div>
          <p>
            Также доступен как <code>wpad.dat</code>:{" "}
            <code>
              http://{ip}:{pacPort}/wpad.dat
            </code>
            .
          </p>
          <ul style={{ lineHeight: "1.7", paddingLeft: "1.25rem" }}>
            <li>
              <strong>Windows:</strong> Параметры → Сеть и интернет → Прокси-сервер → «Автоматическая
              настройка прокси» → «Использовать адрес скрипта» → вставьте URL выше.
            </li>
            <li>
              <strong>Linux (Firefox):</strong> Настройки → Сеть → «Настройка прокси-сервера» →
              «URL автоматической настройки прокси» → вставьте URL.
            </li>
            <li>
              <strong>Android:</strong> Wi-Fi → сеть → «Изменить» → Дополнительно → Прокси: «Авто» /
              «Auto-config» (или «Прокси-скрипт») → URL PAC.
            </li>
          </ul>
          <Message
            severity="info"
            className="w-full"
            text={
              "Исключённые хосты и CIDR из списка исключений получают DIRECT (напрямую, без прокси). " +
              "Переменные окружения http_proxy/https_proxy — это не PAC: PAC действует на уровне браузера/ОС."
            }
          />
        </Card>
      )}

      <Card title="Инструкции по платформам" className="mt-3">
        <TabView>
          <TabPanel header="Windows" leftIcon="pi pi-microsoft">
            <Step n={1}>
              <strong>Установите сертификат CA</strong> (нужен для HTTPS-перехвата). Скачайте{" "}
              <a href="/api/ca.crt" download>
                ca.crt
              </a>
              , затем двойной клик → «Установить сертификат» → «Локальный компьютер» → «Доверенные
              корневые центры сертификации».
            </Step>
            <Step n={2}>
              <strong>Системный прокси:</strong> Параметры → Сеть и интернет → Прокси-сервер →
              «Использовать прокси-сервер» → адрес <code>{ip}</code>, порт <code>{httpPort}</code>.
            </Step>
            <Step n={3}>
              <strong>Или через командную строку (WinHTTP):</strong>
              <Code>{`netsh winhttp set proxy ${ip}:${httpPort}`}</Code>
              Отмена: <Code>{`netsh winhttp reset proxy`}</Code>
            </Step>
            <Step n={4}>
              <strong>PowerShell (переменные окружения):</strong>
              <Code>{`$env:HTTP_PROXY  = "http://${ip}:${httpPort}"\n$env:HTTPS_PROXY = "http://${ip}:${httpsPort}"`}</Code>
            </Step>
            <Step n={5}>
              <strong>SOCKS5</strong> (туннель без кеша) — порт <code>{socksPort}</code>. В Firefox:
              Настройки → Сеть → Ручная настройка прокси → SOCKS Host <code>{ip}</code>, порт{" "}
              <code>{socksPort}</code>, версия SOCKS v5.
            </Step>
          </TabPanel>

          <TabPanel header="Linux" leftIcon="pi pi-desktop">
            <Step n={1}>
              <strong>Установите CA в системный трест:</strong>
              <Code>{`sudo cp ca.crt /usr/local/share/ca-certificates/rustcache-ca.crt\nsudo update-ca-certificates`}</Code>
              Для curl без установки: <Code>{`curl --cacert ca.crt https://example.com/`}</Code>
            </Step>
            <Step n={2}>
              <strong>Переменные окружения (bash/zsh):</strong>
              <Code>{`export http_proxy=http://${ip}:${httpPort}\nexport https_proxy=http://${ip}:${httpsPort}\nexport no_proxy=localhost,127.0.0.1`}</Code>
              Добавьте эти строки в <code>~/.bashrc</code> или <code>~/.zshrc</code>.
            </Step>
            <Step n={3}>
              <strong>GNOME:</strong> Настройки → Сеть → Прокси-сервер → «Вручную» → HTTP{" "}
              <code>
                {ip}:{httpPort}
              </code>
              , HTTPS{" "}
              <code>
                {ip}:{httpsPort}
              </code>
              .
            </Step>
            <Step n={4}>
              <strong>apt:</strong>
              <Code>{`sudo bash -c 'echo "Acquire::http::Proxy \\"http://${ip}:${httpPort}\\";" > /etc/apt/apt.conf.d/99proxy'`}</Code>
            </Step>
            <Step n={5}>
              <strong>SOCKS5</strong> (туннель без кеша) — порт <code>{socksPort}</code>:
              <Code>{`curl --socks5 ${ip}:${socksPort} https://example.com/`}</Code>
            </Step>
          </TabPanel>

          <TabPanel header="Android" leftIcon="pi pi-android">
            <Step n={1}>
              <strong>Установите CA-сертификат:</strong> Скачайте{" "}
              <a href="/api/ca.crt" download>
                ca.crt
              </a>
              . Затем Настройки → Безопасность → Шифрование и учётные данные → «Установить
              сертификат» → «Сертификат CA». (На Android 10+ может потребоваться установка через
              «Пользовательские сертификаты» в настройках безопасности.)
            </Step>
            <Step n={2}>
              <strong>Прокси для Wi-Fi:</strong> Долгое нажатие на подключённую сеть → «Изменить
              сеть» → «Дополнительно» → Прокси: «Вручную» → Имя хоста: <code>{ip}</code>, порт:{" "}
              <code>{httpPort}</code>.
            </Step>
            <Step n={3}>
              <strong>Важно:</strong> Android применяет HTTP-прокси только к браузеру и части
              приложений. Для перехвата HTTPS нужен установленный CA. Приложения, использующие
              certificate pinning, не будут работать через MITM.
            </Step>
            <Step n={4}>
              <strong>SOCKS5</strong> (если нужен туннель) — в приложениях с поддержкой SOCKS5
              укажите{" "}
              <code>
                {ip}:{socksPort}
              </code>
              .
            </Step>
            <Divider />
            <Message
              severity="warn"
              text={`Если устройство и сервер в разных подсетях, убедитесь, что порты ${httpPort}/${httpsPort}/${socksPort} открыты в брандмауэре, и используйте адрес ${ip}.`}
            />
          </TabPanel>
        </TabView>
      </Card>

      <Card title="Проверка подключения" className="mt-3">
        <p style={{ marginTop: 0 }}>После настройки прокси выполните с другого устройства:</p>
        <Code>{`curl -x http://${ip}:${httpPort} http://example.com/`}</Code>
        <p>Два запроса подряд: первый — MISS, второй — HIT. Это означает, что кеш работает.</p>
      </Card>
    </>
  );
}
