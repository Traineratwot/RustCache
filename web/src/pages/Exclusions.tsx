import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Column } from "primereact/column";
import { ConfirmDialog, confirmDialog } from "primereact/confirmdialog";
import { DataTable } from "primereact/datatable";
import { InputText } from "primereact/inputtext";
import { Tag } from "primereact/tag";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useRef, useState } from "react";
import { addExclusion, deleteExclusion, getExclusions } from "../api/client";
import type { Matcher } from "../api/types";

const kindLabel: Record<string, string> = {
  exact: "Точный домен",
  wildcard: "Поддомены (*)",
  suffix: "Суффикс",
  cidr: "CIDR",
};

const kindSeverity: Record<string, "info" | "success" | "warning" | "secondary"> = {
  exact: "info",
  wildcard: "success",
  suffix: "secondary",
  cidr: "warning",
};

export default function Exclusions() {
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
        summary: "Ошибка",
        detail: "Не удалось загрузить исключения",
      });
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleAdd = async () => {
    const d = domain.trim();
    const c = cidr.trim();
    if (!d && !c) {
      toast.current?.show({
        severity: "warn",
        summary: "Внимание",
        detail: "Укажите домен или CIDR",
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
        summary: "Готово",
        detail: "Исключение добавлено",
      });
      await load();
    } catch {
      toast.current?.show({ severity: "error", summary: "Ошибка", detail: "Не удалось добавить" });
    } finally {
      setLoading(false);
    }
  };

  const handleDelete = (m: Matcher) => {
    confirmDialog({
      message: `Удалить исключение «${m.value}»?`,
      header: "Подтверждение",
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
            summary: "Готово",
            detail: "Исключение удалено",
          });
          await load();
        } catch {
          toast.current?.show({
            severity: "error",
            summary: "Ошибка",
            detail: "Не удалось удалить",
          });
        }
      },
    });
  };

  const handleClearAll = () => {
    confirmDialog({
      message: "Удалить все исключения?",
      header: "Подтверждение",
      icon: "pi pi-exclamation-triangle",
      accept: async () => {
        try {
          await deleteExclusion({ all: true });
          toast.current?.show({
            severity: "success",
            summary: "Готово",
            detail: "Все исключения удалены",
          });
          await load();
        } catch {
          toast.current?.show({
            severity: "error",
            summary: "Ошибка",
            detail: "Не удалось удалить",
          });
        }
      },
    });
  };

  return (
    <>
      <Toast ref={toast} />
      <ConfirmDialog />
      <h1 className="page-title">Исключения</h1>
      <Card>
        <p style={{ marginTop: 0 }}>
          Запросы к перечисленным доменам и сетям <strong>не кешируются</strong> и{" "}
          <strong>не проходят через MITM</strong> — соединение идёт напрямую к источнику.
        </p>
        <div className="flex flex-wrap align-items-end gap-2">
          <div className="flex flex-column">
            <label htmlFor="ex-domain" className="mb-1 font-medium">
              Домен
            </label>
            <InputText
              id="ex-domain"
              value={domain}
              onChange={(e) => setDomain(e.target.value)}
              placeholder="example.com или *.example.com"
              style={{ width: "260px" }}
            />
          </div>
          <div className="flex flex-column">
            <label htmlFor="ex-cidr" className="mb-1 font-medium">
              CIDR
            </label>
            <InputText
              id="ex-cidr"
              value={cidr}
              onChange={(e) => setCidr(e.target.value)}
              placeholder="10.0.0.0/8"
              style={{ width: "200px" }}
            />
          </div>
          <Button label="Добавить" icon="pi pi-plus" loading={loading} onClick={handleAdd} />
          {items.length > 0 && (
            <Button
              label="Очистить все"
              icon="pi pi-trash"
              severity="danger"
              outlined
              onClick={handleClearAll}
            />
          )}
        </div>
      </Card>

      <Card title={`Текущие правила (${items.length})`} className="mt-3">
        <DataTable value={items} emptyMessage="Исключений нет" size="small" stripedRows>
          <Column
            field="kind"
            header="Тип"
            body={(m: Matcher) => (
              <Tag
                value={kindLabel[m.kind] ?? m.kind}
                severity={kindSeverity[m.kind] ?? "secondary"}
              />
            )}
            style={{ width: "180px" }}
          />
          <Column field="value" header="Значение" />
          <Column
            header="Действие"
            body={(m: Matcher) => (
              <Button
                icon="pi pi-trash"
                severity="danger"
                text
                tooltip="Удалить"
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
