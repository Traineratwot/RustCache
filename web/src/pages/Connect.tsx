import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Divider } from "primereact/divider";
import { Message } from "primereact/message";
import { TabPanel, TabView } from "primereact/tabview";
import { Tag } from "primereact/tag";
import { Toast } from "primereact/toast";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { getConfig, getNetInfo, getPac } from "../api/client";
import type { Config, PacInfo } from "../api/types";
import { DEFAULT_PORTS } from "../lib/constants";

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

function Step({ n, children }: { n: number; children: ReactNode }) {
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
        const text = (await r.text()).trim();
        if (/^\d{1,3}(\.\d{1,3}){3}$/.test(text) || text.includes(":")) return text;
      }
    } catch {
      // try next
    }
  }
  return null;
}

/** Connect: client setup instructions and detected LAN/external IPs. */
export default function Connect() {
  const { t } = useTranslation();
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
          summary: t("common.copied"),
          detail: text,
          life: 1500,
        }),
      )
      .catch(() => {
        /* clipboard may be unavailable */
      });
  };

  const httpPort = cfg?.http.port ?? DEFAULT_PORTS.http;
  const httpsPort = cfg?.https.port ?? DEFAULT_PORTS.https;
  const socksPort = cfg?.socks5.port ?? DEFAULT_PORTS.socks5;
  const ip = selectedIp;
  const pacPort = pac?.port ?? DEFAULT_PORTS.pac;
  const pacUrl = `http://${ip}:${pacPort}/proxy.pac`;
  const pacModeLabel =
    pac?.mode === "http"
      ? t("connect.pacModeHttp")
      : pac?.mode === "socks"
        ? t("connect.pacModeSocks")
        : t("connect.pacModeBoth");

  return (
    <>
      <Toast ref={toast} />
      <h1 className="page-title">{t("connect.title")}</h1>
      <Message severity="info" text={t("connect.intro")} className="mb-3 w-full" />

      <h2 style={{ fontSize: "1.1rem", margin: "0 0 0.75rem" }}>{t("connect.chooseAddr")}</h2>
      <div className="stat-grid">
        {localIps.map((lip) => (
          <IpCard
            key={lip}
            label={lip.startsWith("172.") ? t("connect.dockerIp") : t("connect.lanIp")}
            ip={lip}
            selected={selectedIp === lip}
            onSelect={() => setSelectedIp(lip)}
          />
        ))}
        <IpCard
          label={t("connect.loopback")}
          ip="127.0.0.1"
          selected={selectedIp === "127.0.0.1"}
          onSelect={() => setSelectedIp("127.0.0.1")}
        />
        {externalIp && (
          <IpCard
            label={t("connect.external")}
            ip={externalIp}
            selected={selectedIp === externalIp}
            onSelect={() => setSelectedIp(externalIp)}
          />
        )}
      </div>

      <div className="flex gap-2 mt-2">
        <Button label={t("connect.refreshIp")} icon="pi pi-refresh" text onClick={refresh} />
        <Button label={t("connect.copySelected")} icon="pi pi-copy" text onClick={() => copy(ip)} />
      </div>

      <Card title={t("connect.addrTitle", { ip })} className="mt-3">
        <p style={{ marginTop: 0 }}>
          {t("connect.httpProxy")}: <Tag value={`${ip}:${httpPort}`} severity="info" /> ·{" "}
          {t("connect.httpsMitm")}: <Tag value={`${ip}:${httpsPort}`} severity="info" /> ·{" "}
          {t("connect.socks5")}: <Tag value={`${ip}:${socksPort}`} severity="secondary" />
        </p>
      </Card>

      {pac?.enabled !== false && (
        <Card title={t("connect.pacTitle")} className="mt-3">
          <p style={{ marginTop: 0 }}>{t("connect.pacUrlLabel", { mode: pacModeLabel })}</p>
          <Code>{pacUrl}</Code>
          <div className="flex gap-2 mb-3">
            <Button
              label={t("connect.copyPacUrl")}
              icon="pi pi-copy"
              text
              onClick={() => copy(pacUrl)}
            />
          </div>
          <p>
            <Trans
              i18nKey="connect.wpadAlso"
              values={{ url: `http://${ip}:${pacPort}/wpad.dat` }}
              components={{ 1: <code />, 2: <code /> }}
            />
          </p>
          <ul style={{ lineHeight: "1.7", paddingLeft: "1.25rem" }}>
            <li>
              <strong>Windows:</strong> {t("connect.pacWin")}
            </li>
            <li>
              <strong>Linux (Firefox):</strong> {t("connect.pacLinux")}
            </li>
            <li>
              <strong>macOS:</strong> {t("connect.pacMac")}
            </li>
            <li>
              <strong>iPhone / iOS:</strong> {t("connect.pacIos")}
            </li>
            <li>
              <strong>Android:</strong> {t("connect.pacAndroid")}
            </li>
          </ul>
          <Message severity="info" className="w-full" text={t("connect.pacExclusionsNote")} />
        </Card>
      )}

      <Card title={t("connect.platforms")} className="mt-3">
        <TabView>
          <TabPanel header={t("connect.windows")} leftIcon="pi pi-microsoft">
            <Step n={1}>
              <Trans
                i18nKey="connect.win1"
                components={{
                  1: (
                    <a href="/api/ca.crt" download>
                      ca.crt
                    </a>
                  ),
                }}
              />
            </Step>
            <Step n={2}>
              <Trans
                i18nKey="connect.win2"
                values={{ ip, port: httpPort }}
                components={{ 1: <code />, 2: <code /> }}
              />
            </Step>
            <Step n={3}>
              {t("connect.win3")}
              <Code>{`netsh winhttp set proxy ${ip}:${httpPort}`}</Code>
              {t("connect.win3cancel")} <Code>{`netsh winhttp reset proxy`}</Code>
            </Step>
            <Step n={4}>
              {t("connect.win4")}
              <Code>{`$env:HTTP_PROXY  = "http://${ip}:${httpPort}"\n$env:HTTPS_PROXY = "http://${ip}:${httpsPort}"`}</Code>
            </Step>
            <Step n={5}>
              <Trans
                i18nKey="connect.win5"
                values={{ ip, port: socksPort }}
                components={{ 1: <code />, 2: <code />, 3: <code /> }}
              />
            </Step>
          </TabPanel>

          <TabPanel header={t("connect.linux")} leftIcon="pi pi-desktop">
            <Step n={1}>
              {t("connect.lin1")}
              <Code>{`sudo cp ca.crt /usr/local/share/ca-certificates/rustcache-ca.crt\nsudo update-ca-certificates`}</Code>
              {t("connect.lin1curl")} <Code>{`curl --cacert ca.crt https://example.com/`}</Code>
            </Step>
            <Step n={2}>
              {t("connect.lin2")}
              <Code>{`export http_proxy=http://${ip}:${httpPort}\nexport https_proxy=http://${ip}:${httpsPort}\nexport no_proxy=localhost,127.0.0.1`}</Code>
              <Trans i18nKey="connect.lin2hint" components={{ 1: <code />, 2: <code /> }} />
            </Step>
            <Step n={3}>
              <Trans
                i18nKey="connect.lin3"
                values={{ http: `${ip}:${httpPort}`, https: `${ip}:${httpsPort}` }}
                components={{ 1: <code />, 2: <code /> }}
              />
            </Step>
            <Step n={4}>
              {t("connect.lin4")}
              <Code>{`sudo bash -c 'echo "Acquire::http::Proxy \\"http://${ip}:${httpPort}\\";" > /etc/apt/apt.conf.d/99proxy'`}</Code>
            </Step>
            <Step n={5}>
              <Trans
                i18nKey="connect.lin5"
                values={{ port: socksPort }}
                components={{ 1: <code /> }}
              />
              <Code>{`curl --socks5 ${ip}:${socksPort} https://example.com/`}</Code>
            </Step>
          </TabPanel>

          <TabPanel header={t("connect.macos")} leftIcon="pi pi-apple">
            <Step n={1}>
              <Trans
                i18nKey="connect.mac1"
                components={{
                  1: (
                    <a href="/api/ca.crt" download>
                      ca.crt
                    </a>
                  ),
                }}
              />
              <Code>{`sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain ca.crt`}</Code>
              {t("connect.mac1undo")}{" "}
              <Code>{`sudo security delete-certificate -c "RustCache CA" /Library/Keychains/System.keychain`}</Code>
            </Step>
            <Step n={2}>
              <Trans
                i18nKey="connect.mac2"
                values={{ ip, port: httpPort }}
                components={{ 1: <code />, 2: <code /> }}
              />
            </Step>
            <Step n={3}>
              {t("connect.mac3")}
              <Code>{`sudo networksetup -setwebproxy "Wi-Fi" ${ip} ${httpPort}\nsudo networksetup -setsecurewebproxy "Wi-Fi" ${ip} ${httpPort}`}</Code>
              {t("connect.mac3cancel")}{" "}
              <Code>{`sudo networksetup -setwebproxystate "Wi-Fi" off\nsudo networksetup -setsecurewebproxystate "Wi-Fi" off`}</Code>
            </Step>
            <Step n={4}>
              {t("connect.mac4")}
              <Code>{`export http_proxy=http://${ip}:${httpPort}\nexport https_proxy=http://${ip}:${httpsPort}\nexport no_proxy=localhost,127.0.0.1`}</Code>
            </Step>
            <Step n={5}>
              <Trans
                i18nKey="connect.mac5"
                values={{ port: socksPort }}
                components={{ 1: <code /> }}
              />
              <Code>{`sudo networksetup -setsocksfirewallproxy "Wi-Fi" ${ip} ${socksPort}`}</Code>
              <Trans
                i18nKey="connect.mac5firefox"
                values={{ ip, port: socksPort }}
                components={{ 1: <code />, 2: <code /> }}
              />
            </Step>
            <Divider />
            <Message severity="info" className="w-full" text={t("connect.macNote")} />
          </TabPanel>

          <TabPanel header={t("connect.ios")} leftIcon="pi pi-mobile">
            <Step n={1}>
              <Trans
                i18nKey="connect.ios1"
                components={{
                  1: (
                    <a href="/api/ca.crt" download>
                      ca.crt
                    </a>
                  ),
                }}
              />
            </Step>
            <Step n={2}>
              <Trans
                i18nKey="connect.ios2"
                values={{ ip, port: httpPort }}
                components={{ 1: <code />, 2: <code /> }}
              />
            </Step>
            <Step n={3}>
              <Trans
                i18nKey="connect.ios3"
                values={{ ip, port: pacPort }}
                components={{ 1: <code /> }}
              />
            </Step>
            <Step n={4}>{t("connect.ios4")}</Step>
            <Divider />
            <Message
              severity="warn"
              text={t("connect.andWarn", {
                ports: `${httpPort}/${httpsPort}/${socksPort}`,
                ip,
              })}
            />
          </TabPanel>

          <TabPanel header={t("connect.android")} leftIcon="pi pi-android">
            <Step n={1}>
              <Trans
                i18nKey="connect.and1"
                components={{
                  1: (
                    <a href="/api/ca.crt" download>
                      ca.crt
                    </a>
                  ),
                }}
              />
            </Step>
            <Step n={2}>
              <Trans
                i18nKey="connect.and2"
                values={{ ip, port: httpPort }}
                components={{ 1: <code />, 2: <code /> }}
              />
            </Step>
            <Step n={3}>{t("connect.and3")}</Step>
            <Step n={4}>
              <Trans
                i18nKey="connect.and4"
                values={{ ip: `${ip}:${socksPort}` }}
                components={{ 1: <code /> }}
              />
            </Step>
            <Divider />
            <Message
              severity="warn"
              text={t("connect.andWarn", {
                ports: `${httpPort}/${httpsPort}/${socksPort}`,
                ip,
              })}
            />
          </TabPanel>
        </TabView>
      </Card>

      <Card title={t("connect.checkTitle")} className="mt-3">
        <p style={{ marginTop: 0 }}>{t("connect.checkIntro")}</p>
        <Code>{`curl -x http://${ip}:${httpPort} http://example.com/`}</Code>
        <p>{t("connect.checkHint")}</p>
      </Card>
    </>
  );
}
