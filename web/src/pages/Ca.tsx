import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Message } from "primereact/message";
import { Toast } from "primereact/toast";
import { useEffect, useRef, useState } from "react";
import { caFingerprint, downloadCa } from "../api/client";

export default function Ca() {
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
        if (alive) setError("Не удалось получить сертификат CA");
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

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
        summary: "Готово",
        detail: "Файл ca.crt сохранён",
      });
    } catch {
      toast.current?.show({
        severity: "error",
        summary: "Ошибка",
        detail: "Не удалось скачать сертификат",
      });
    } finally {
      setLoading(false);
    }
  };

  return (
    <>
      <Toast ref={toast} />
      <h1 className="page-title">Сертификат CA</h1>

      <Message
        severity="warn"
        text="MITM-перехват HTTPS — явное действие оператора. RustCache выпускает поддельные сертификаты для посещаемых сайтов, подписанные этим корневым CA. Доверять этому CA следует только на машинах, где вы осознанно запускаете прокси."
        className="mb-3 w-full"
      />

      <Card title="Как установить доверие">
        <ol style={{ margin: 0, paddingLeft: "1.25rem", lineHeight: "1.7" }}>
          <li>
            Скачайте файл <code>ca.crt</code> кнопкой ниже.
          </li>
          <li>
            <strong>Linux (системный):</strong> положите в{" "}
            <code>/usr/local/share/ca-certificates/rustcache-ca.crt</code> и выполните{" "}
            <code>sudo update-ca-certificates</code>.
          </li>
          <li>
            <strong>Firefox:</strong> Настройки → Приватность → Сертификаты → Импортируйте ca.crt и
            отметьте «Доверять этому CA для идентификации сайтов».
          </li>
          <li>
            <strong>Chrome / системный трест:</strong> используйте системное хранилище (см. выше).
          </li>
          <li>
            Для <code>curl</code> можно передать <code>--cacert ca.crt</code> без установки в трест.
          </li>
        </ol>
      </Card>

      <Card title="Отпечаток (SHA-256)" className="mt-3">
        {error ? (
          <Message severity="error" text={error} />
        ) : fingerprint ? (
          <>
            <p style={{ marginTop: 0 }}>Сверьте отпечаток перед установкой доверия:</p>
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
          <p>Вычисление отпечатка...</p>
        )}
      </Card>

      <Card title="Скачать" className="mt-3">
        <Button
          label="Скачать ca.crt"
          icon="pi pi-download"
          loading={loading}
          onClick={handleDownload}
        />
      </Card>
    </>
  );
}
