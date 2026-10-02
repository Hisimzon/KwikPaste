import { type GetRef, Input, Modal } from "antd";
import type { ChangeEvent, FC } from "react";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { pairLanDevice } from "@/commands";
import type { LanNearbyDevice, LanPairTarget } from "@/types/sync";

const PAIRING_CODE_LENGTH = 6;

export type LanPairModalTarget =
  | { kind: "device"; device: LanNearbyDevice }
  | { kind: "manual" };

interface LanPairModalProps {
  target: LanPairModalTarget | null;
  onClose: () => void;
}

/**
 * 输入对方设备上显示的配对码完成配对；手动添加时还要填对方地址。
 */
const LanPairModal: FC<LanPairModalProps> = (props) => {
  const { t } = useTranslation(["preferences", "common"]);
  const { target, onClose } = props;
  const [code, setCode] = useState("");
  const [address, setAddress] = useState("");
  const [pairing, setPairing] = useState(false);
  const codeRef = useRef<GetRef<typeof Input.OTP>>(null);
  const addressRef = useRef<GetRef<typeof Input>>(null);
  const manual = target?.kind === "manual";
  const submitDisabled =
    code.length !== PAIRING_CODE_LENGTH ||
    (manual && address.trim().length === 0);
  const title =
    target?.kind === "device"
      ? t("lanSync.pairModal.title", { name: target.device.name })
      : t("lanSync.pairModal.manualTitle");

  const close = () => {
    setCode("");
    setAddress("");
    onClose();
  };

  const pair = async () => {
    if (!target || submitDisabled) return;

    const pairTarget: LanPairTarget =
      target.kind === "device"
        ? { deviceId: target.device.id }
        : { address: address.trim() };

    setPairing(true);
    try {
      await pairLanDevice(pairTarget, code);
      close();
    } catch {
      // 错误 toast 已由 commands 层统一处理；清空配对码方便重新输入。
      setCode("");
      codeRef.current?.focus();
    } finally {
      setPairing(false);
    }
  };

  const handleAfterOpenChange = (open: boolean) => {
    if (!open) return;

    if (manual) {
      addressRef.current?.focus();
      return;
    }

    codeRef.current?.focus();
  };

  const handleAddressChange = (event: ChangeEvent<HTMLInputElement>) => {
    setAddress(event.target.value);
  };

  const handleCodeInput = (values: string[]) => {
    setCode(values.join(""));
  };

  return (
    <Modal
      afterOpenChange={handleAfterOpenChange}
      cancelText={t("common:actions.cancel")}
      confirmLoading={pairing}
      destroyOnHidden
      okButtonProps={{ disabled: submitDisabled }}
      okText={t("lanSync.pairModal.ok")}
      onCancel={close}
      onOk={pair}
      open={target !== null}
      title={title}
    >
      <div className="flex flex-col gap-4 pt-2">
        {manual ? (
          <div className="flex flex-col gap-1.5">
            <span className="text-sm">{t("lanSync.pairModal.address")}</span>
            <Input
              onChange={handleAddressChange}
              onPressEnter={pair}
              placeholder={t("lanSync.pairModal.addressPlaceholder")}
              ref={addressRef}
              value={address}
            />
          </div>
        ) : null}

        <div className="flex flex-col gap-1.5">
          <span className="text-sm">{t("lanSync.pairModal.code")}</span>
          <Input.OTP
            formatter={keepDigits}
            length={PAIRING_CODE_LENGTH}
            onInput={handleCodeInput}
            ref={codeRef}
            value={code}
          />
          <span className="text-ant-secondary text-xs">
            {t("lanSync.pairModal.codeHint")}
          </span>
        </div>
      </div>
    </Modal>
  );
};

/**
 * 配对码只有数字，其余字符直接丢掉。
 */
function keepDigits(value: string) {
  return value.replace(/\D/g, "");
}

export default LanPairModal;
