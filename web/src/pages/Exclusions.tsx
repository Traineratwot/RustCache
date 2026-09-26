import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Column } from "primereact/column";
import { ConfirmDialog, confirmDialog } from "primereact/confirmdialog";
import { DataTable } from "primereact/datatable";
import { InputText } from "primereact/inputtext";
import { Tag } from "primereact/tag";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { addExclusion, deleteExclusion, getExclusions } from "../api/client";
import type { Matcher } from "../api/types";

const kindKey: Record<string, string> = {
  exact: "exclusions.kindExact",
  wildcard: "exclusions.kindWildcard",
  suffix: "exclusions.kindSuffix",
  cidr: "exclusions.kindCidr",
};

const kindSeverity: Record<string, "info" | "success" | "warning" | "secondary"> = {
  exact: "info",
  wildcard: "success",
  suffix: "secondary",
  cidr: "warning",
};

export default function Exclusions() {
  const { t } = useTranslation();
  const [items, setItems] = useState<Matcher[]>([]);
  const [domain, setDomain] = useState("");
  const [cidr, setCidr] = useState("");
  const [loading, setLoading] = useState(false);
  const toast = useRef<Toast>(null);

  const load = useCallback(async () => {
    try {
      const r = await getExclusions();
      setItems(r.exclusions);
    } catch {
      toast.current?.show({
        severity: "error",
        summary: t("common.error"),
        detail: t("exclusions.loadError"),
      });
    }
  }, [t]);

  useEffect(() => {
    load();
  }, [load]);

  const handleAdd = async () => {
    const d = domain.trim();
    const c = cidr.trim();
    if (!d && !c) {
      toast.current?.show({
        severity: "warn",
        summary: t("common.warning"),
        detail: t("exclusions.needInput"),
      });
      return;
    }
    setLoading(true);
    try {
      await addExclusion({
        ...(d ? { domain: d } : {}),
        ...(c ? { cidr: c } : {}),
      });
      setDomain("");
      setCidr("");
      toast.current?.show({
        severity: "success",
        summary: t("common.done"),
        detail: t("exclusions.added"),
      });
      await load();
    } catch {
      toast.current?.show({
        severity: "error",
        summary: t("common.error"),
        detail: t("exclusions.addError"),
      });
    } finally {
      setLoading(false);
    }
  };

  const handleDelete = (m: Matcher) => {
    confirmDialog({
      message: t("exclusions.deleteOne", { value: m.value }),
      header: t("common.confirmTitle"),
      icon: "pi pi-exclamation-triangle",
      accept: async () => {
        try {
          if (m.kind === "cidr") {
            await deleteExclusion({ cidr: m.value });
          } else {
            await deleteExclusion({ domain: m.value });
          }
          toast.current?.show({
            severity: "success",
            summary: t("common.done"),
            detail: t("exclusions.deleted"),
          });
          await load();
        } catch {
          toast.current?.show({
            severity: "error",
            summary: t("common.error"),
            detail: t("exclusions.deleteError"),
          });
        }
      },
    });
  };

  const handleClearAll = () => {
    confirmDialog({
      message: t("exclusions.deleteAll"),
      header: t("common.confirmTitle"),
      icon: "pi pi-exclamation-triangle",
      accept: async () => {
        try {
          await deleteExclusion({ all: true });
          toast.current?.show({
            severity: "success",
            summary: t("common.done"),
            detail: t("exclusions.deletedAll"),
          });
          await load();
        } catch {
          toast.current?.show({
            severity: "error",
            summary: t("common.error"),
            detail: t("exclusions.deleteError"),
          });
        }
      },
    });
  };

  return (
    <>
      <Toast ref={toast} />
      <ConfirmDialog />
      <h1 className="page-title">{t("exclusions.title")}</h1>
      <Card>
        <p style={{ marginTop: 0 }}>
          <Trans i18nKey="exclusions.description" components={{ 1: <strong />, 2: <strong /> }} />
        </p>
        <div className="flex flex-wrap align-items-end gap-2">
          <div className="flex flex-column">
            <label htmlFor="ex-domain" className="mb-1 font-medium">
              {t("exclusions.addDomain")}
            </label>
            <InputText
              id="ex-domain"
              value={domain}
              onChange={(e) => setDomain(e.target.value)}
              placeholder={t("exclusions.domainPlaceholder")}
              style={{ width: "260px" }}
            />
          </div>
          <div className="flex flex-column">
            <label htmlFor="ex-cidr" className="mb-1 font-medium">
              {t("exclusions.addCidr")}
            </label>
            <InputText
              id="ex-cidr"
              value={cidr}
              onChange={(e) => setCidr(e.target.value)}
              placeholder={t("exclusions.cidrPlaceholder")}
              style={{ width: "200px" }}
            />
          </div>
          <Button
            label={t("exclusions.add")}
            icon="pi pi-plus"
            loading={loading}
            onClick={handleAdd}
          />
          {items.length > 0 && (
            <Button
              label={t("common.clearAll")}
              icon="pi pi-trash"
              severity="danger"
              outlined
              onClick={handleClearAll}
            />
          )}
        </div>
      </Card>

      <Card title={t("exclusions.current", { count: items.length })} className="mt-3">
        <DataTable value={items} emptyMessage={t("exclusions.empty")} size="small" stripedRows>
          <Column
            field="kind"
            header={t("exclusions.kind")}
            body={(m: Matcher) => (
              <Tag
                value={kindKey[m.kind] ? t(kindKey[m.kind]) : m.kind}
                severity={kindSeverity[m.kind] ?? "secondary"}
              />
            )}
            style={{ width: "180px" }}
          />
          <Column field="value" header={t("exclusions.value")} />
          <Column
            header={t("exclusions.action")}
            body={(m: Matcher) => (
              <Button
                icon="pi pi-trash"
                severity="danger"
                text
                tooltip={t("exclusions.deleteTooltip")}
                onClick={() => handleDelete(m)}
              />
            )}
            style={{ width: "100px" }}
          />
        </DataTable>
      </Card>
    </>
  );
}
