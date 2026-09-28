import { message as staticMessage, Modal as staticModal } from "antd";
import type { MessageInstance } from "antd/es/message/interface";
import { createElement, Fragment } from "react";
import ModalKeyboardLayer from "@/components/ModalKeyboardLayer";

type ModalConfirmApi = Pick<typeof staticModal, "confirm">;

let messageApi: MessageInstance = staticMessage;
let modalApi: ModalConfirmApi = staticModal;

/**
 * 确认框随内容压入弹窗键盘层：Enter / Esc 只作用于确认框，不会落到列表去粘贴当前条目或隐藏窗口。
 */
const confirmWithKeyboardLayer: ModalConfirmApi["confirm"] = (props) => {
  return modalApi.confirm({
    ...props,
    content: createElement(
      Fragment,
      null,
      createElement(ModalKeyboardLayer),
      props.content,
    ),
  });
};

export function setMessageApi(api: MessageInstance): void {
  messageApi = api;
}

export function getMessageApi(): MessageInstance {
  return messageApi;
}

export function setModalApi(api: ModalConfirmApi): void {
  modalApi = api;
}

export function getModalApi(): ModalConfirmApi {
  return { confirm: confirmWithKeyboardLayer };
}
