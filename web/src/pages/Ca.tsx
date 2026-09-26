import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Message } from "primereact/message";
import { Toast } from "primereact/toast";
import { useEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { downloadCa } from "../api/client";
import { caFingerprint } from "../lib/crypto";

/** CA: show root cert fingerprint and download `ca.crt` for trust install. */
export default function Ca() {
  const { t } = useTranslation();
  const [fingerprint, setFingerprint] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const toast = useRef<Toast>(null);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const pem = await downloadCa();
        const fp = await caFingerprint(pem);
        if (alive) setFingerprint(fp);
      } catch {
        if (alive) setError(t("ca.fingerprintError"));
      }
    })();
    return () => {
      alive = false;
    };
  }, [t]);

  const handleDownload = async () => {
    setLoading(true);
    try {
      const pem = await downloadCa();
      const blob = new Blob([pem], { type: "application/x-pem-file" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = "ca.crt";
      document.body.appendChild(a);
      a.click();
      a.remove();
      URL.revokeObjectURL(url);
      toast.current?.show({
        severity: "success",
        summary: t("common.done"),
        detail: t("ca.saved"),
      });
    } catch {
      toast.current?.show({
        severity: "error",
        summary: t("common.error"),
        detail: t("ca.saveError"),
      });
    } finally {
      setLoading(false);
    }
  };

  return (
    <>
      <Toast ref={toast} />
      <h1 className="page-title">{t("ca.title")}</h1>

      <Message severity="warn" text={t("ca.warn")} className="mb-3 w-full" />

      <Card title={t("ca.trustTitle")}>
        <ol style={{ margin: 0, paddingLeft: "1.25rem", lineHeight: "1.7" }}>
          <li>
            <Trans i18nKey="ca.stepDownload" components={{ 1: <code /> }} />
          </li>
          <li>
            <Trans
              i18nKey="ca.stepLinux"
              components={{
                1: <strong />,
                2: <code />,
                3: <code />,
              }}
            />
          </li>
          <li>
            <Trans i18nKey="ca.stepFirefox" components={{ 1: <strong /> }} />
          </li>
          <li>
            <Trans i18nKey="ca.stepChrome" components={{ 1: <strong /> }} />
          </li>
          <li>
            <Trans i18nKey="ca.stepCurl" components={{ 1: <code />, 2: <code /> }} />
          </li>
        </ol>
      </Card>

      <Card title={t("ca.fingerprint")} className="mt-3">
        {error ? (
          <Message severity="error" text={error} />
        ) : fingerprint ? (
          <>
            <p style={{ marginTop: 0 }}>{t("ca.fingerprintHint")}</p>
            <code
              style={{
                display: "block",
                padding: "0.75rem",
                background: "var(--surface-ground)",
                borderRadius: "var(--border-radius)",
                fontSize: "0.95rem",
                wordBreak: "break-all",
              }}
            >
              {fingerprint}
            </code>
          </>
        ) : (
          <p>{t("ca.computing")}</p>
        )}
      </Card>

      <Card title={t("ca.downloadTitle")} className="mt-3">
        <Button
          label={t("ca.downloadCrt")}
          icon="pi pi-download"
          loading={loading}
          onClick={handleDownload}
        />
      </Card>
    </>
  );
}
